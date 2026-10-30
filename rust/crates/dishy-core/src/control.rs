//! The device maintenance writes, separated from the transport that sends them.
//!
//! Every action is one-shot and user-initiated. The recorder thread is the only
//! owner of the clients, so this module names the request field and body and
//! lets the caller pick the dish or router client; it never polls.
//!
//! The request bodies are copied from `core/dishClient.ts`, which itself follows
//! the schema: `DishFactoryResetRequest` carries `app_reset = true`, and
//! `DishStowRequest` carries `unstow = true` only when unfolding. Everything else
//! is an empty sub-message.

use dishy_proto::fields;

/// Which box a maintenance action talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlTarget {
    Dish,
    Router,
}

/// A maintenance write. These are deliberately not a poll and are never queued
/// automatically; a host sends one only after the user confirms it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    ClearObstructionMap,
    RebootDish,
    FactoryResetDish,
    /// `unstow = true` unfolds the dish; `false` folds it flat.
    StowDish {
        unstow: bool,
    },
    RebootRouter,
    FactoryResetRouter,
}

impl ControlAction {
    // SPEC-CTRL-003: the dish's clear/reboot/factory-reset/stow controls.
    // SPEC-CTRL-004: the router's reboot/factory-reset controls.
    pub const fn target(self) -> ControlTarget {
        match self {
            Self::ClearObstructionMap
            | Self::RebootDish
            | Self::FactoryResetDish
            | Self::StowDish { .. } => ControlTarget::Dish,
            Self::RebootRouter | Self::FactoryResetRouter => ControlTarget::Router,
        }
    }

    /// The `SpaceX.API.Device.Request` oneof field.
    pub const fn field(self) -> u32 {
        match self {
            Self::ClearObstructionMap => fields::DISH_CLEAR_OBSTRUCTION_MAP,
            Self::RebootDish | Self::RebootRouter => fields::REBOOT,
            Self::FactoryResetDish => fields::DISH_FACTORY_RESET,
            Self::StowDish { .. } => fields::DISH_STOW,
            Self::FactoryResetRouter => fields::FACTORY_RESET,
        }
    }

    /// The sub-message body, exactly as the TypeScript client sends it.
    pub const fn body(self) -> &'static [u8] {
        match self {
            Self::FactoryResetDish => &[0x08, 0x01],
            Self::StowDish { unstow: true } => &[0x08, 0x01],
            Self::ClearObstructionMap
            | Self::RebootDish
            | Self::StowDish { unstow: false }
            | Self::RebootRouter
            | Self::FactoryResetRouter => &[],
        }
    }

    /// A command that drops service or wipes state. The page styles these as
    /// danger actions and confirms all of them; this flag is for that styling.
    pub const fn destructive(self) -> bool {
        !matches!(self, Self::ClearObstructionMap)
    }

    /// Stow is only meaningful on a dish that reports actuators.
    pub const fn requires_actuators(self) -> bool {
        matches!(self, Self::StowDish { .. })
    }

    /// Every action needs the same explicit confirmation, including the ones
    /// that only restart a radio.
    // SPEC-CTRL-006: no single click may send a reboot, reset, or stow change.
    pub const fn needs_confirmation(self) -> bool {
        true
    }

    // SPEC-CTRL-006: the toast names what was actually sent.
    pub const fn success_key(self) -> &'static str {
        match self {
            Self::ClearObstructionMap => "settings.resetObstructionSent",
            Self::RebootDish => "settings.rebootDishSent",
            Self::FactoryResetDish => "settings.factoryResetDishSent",
            Self::StowDish { unstow: false } => "settings.stowSent",
            Self::StowDish { unstow: true } => "settings.unstowSent",
            Self::RebootRouter => "settings.rebootRouterSent",
            Self::FactoryResetRouter => "settings.factoryResetRouterSent",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-CTRL-003, SPEC-CTRL-004: each action names its target and the request
    // field the TypeScript client uses.
    #[test]
    fn actions_map_to_the_shared_request_fields() {
        assert_eq!(
            ControlAction::ClearObstructionMap.target(),
            ControlTarget::Dish
        );
        assert_eq!(ControlAction::ClearObstructionMap.field(), 2017);
        assert_eq!(ControlAction::RebootDish.field(), 1001);
        assert_eq!(ControlAction::FactoryResetDish.field(), 2021);
        assert_eq!(ControlAction::StowDish { unstow: false }.field(), 2002);
        assert_eq!(ControlAction::RebootRouter.target(), ControlTarget::Router);
        assert_eq!(ControlAction::RebootRouter.field(), 1001);
        assert_eq!(ControlAction::FactoryResetRouter.field(), 1011);
    }

    // SPEC-CTRL-003: the dish factory reset asks for an app reset, and stow's
    // one boolean is set only when unfolding.
    #[test]
    fn request_bodies_match_the_typescript_client() {
        assert_eq!(
            ControlAction::FactoryResetDish.body(),
            [0x08, 0x01].as_slice()
        );
        assert!(ControlAction::StowDish { unstow: false }.body().is_empty());
        assert_eq!(
            ControlAction::StowDish { unstow: true }.body(),
            &[0x08, 0x01]
        );
        assert!(ControlAction::ClearObstructionMap.body().is_empty());
        assert!(ControlAction::RebootRouter.body().is_empty());
    }

    // SPEC-CTRL-003: stow is the one action that depends on the dish reporting
    // actuators; the page hides it otherwise.
    #[test]
    fn only_stow_depends_on_actuators() {
        assert!(ControlAction::StowDish { unstow: false }.requires_actuators());
        assert!(ControlAction::StowDish { unstow: true }.requires_actuators());
        assert!(!ControlAction::RebootDish.requires_actuators());
        assert!(!ControlAction::RebootRouter.requires_actuators());
    }

    // SPEC-CTRL-006, SPEC-SAFETY-004: every write is confirmed, and each action
    // reports the phrase for what it actually did.
    #[test]
    fn confirmation_and_success_copy_cover_every_action() {
        let actions = [
            ControlAction::ClearObstructionMap,
            ControlAction::RebootDish,
            ControlAction::FactoryResetDish,
            ControlAction::StowDish { unstow: false },
            ControlAction::StowDish { unstow: true },
            ControlAction::RebootRouter,
            ControlAction::FactoryResetRouter,
        ];
        for action in actions {
            assert!(action.needs_confirmation());
            assert!(!action.success_key().is_empty());
        }
        assert_eq!(
            ControlAction::StowDish { unstow: false }.success_key(),
            "settings.stowSent"
        );
        assert_eq!(
            ControlAction::StowDish { unstow: true }.success_key(),
            "settings.unstowSent"
        );
        assert!(ControlAction::FactoryResetRouter.destructive());
        assert!(!ControlAction::ClearObstructionMap.destructive());
    }
}
