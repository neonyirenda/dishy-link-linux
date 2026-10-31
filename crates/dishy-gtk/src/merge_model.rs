//! The words of the duplicate-device prompt on the Data Usage page
//! (spec 0176). Port of upstream `components/shared/DeviceMergePrompt.tsx`.

use dishy_core::odometer::{ClientTotal, MergeCandidate, key_of};

use crate::i18n::Translator;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCard {
    pub name: String,
    /// "Apple · Active now" / "Private · 2 h ago".
    pub detail: String,
    /// "Router ID: 7".
    pub router_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePrompt {
    pub title: String,
    pub more: Option<String>,
    /// Pango markup with the shared name in bold.
    pub explanation: String,
    pub older: MergeCard,
    pub newer: MergeCard,
    pub outcome: String,
    pub same: String,
    pub different: String,
}

/// The prompt for the first proposal, or `None` when there is none or its
/// records are no longer listed.
pub fn merge_prompt(
    candidates: &[MergeCandidate],
    totals: &[ClientTotal],
    now_ms: i64,
    translator: &Translator,
) -> Option<MergePrompt> {
    let candidate = candidates.first()?;
    let find = |key: &str| {
        totals
            .iter()
            .find(|total| key_of(total.client_id, &total.mac_address) == key)
    };
    let (older, newer) = (find(&candidate.from_key)?, find(&candidate.to_key)?);
    let card = |total: &ClientTotal| {
        let vendor = dishy_core::oui::OuiRegistry::bundled()
            .vendor_for_mac(&total.mac_address)
            .map(str::to_owned);
        let maker = vendor
            .clone()
            .unwrap_or_else(|| translator.text("devices.privateVendor"));
        let seen = if now_ms - total.last_seen_ms < 120_000 {
            translator.text("devices.activeNow")
        } else {
            crate::dashboard::format_relative_age(now_ms - total.last_seen_ms, translator)
        };
        MergeCard {
            name: total
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .or(vendor)
                .unwrap_or_else(|| total.mac_address.clone()),
            detail: format!("{maker} · {seen}"),
            router_id: translator.text_with(
                "devices.routerId",
                &[(
                    "id",
                    &total
                        .client_id
                        .map_or_else(|| translator.text("common.unknown"), |id| id.to_string()),
                )],
            ),
        }
    };
    // The catalog wraps the name in <name>…</name>; escape everything, then
    // turn that one tag into bold.
    let explanation = gtk::glib::markup_escape_text(
        &translator.text_with("merge.explanation", &[("name", &candidate.detail)]),
    )
    .replace("&lt;name&gt;", "<b>")
    .replace("&lt;/name&gt;", "</b>");
    let outcome = if candidate.folds_bytes {
        translator.text_with(
            "merge.combinesTotal",
            &[(
                "total",
                &crate::format::format_bytes(candidate.result_rx_bytes + candidate.result_tx_bytes),
            )],
        )
    } else {
        translator.text("merge.differentMonths")
    };
    Some(MergePrompt {
        title: translator.text("merge.title"),
        more: (candidates.len() > 1).then(|| {
            translator.text_with(
                "merge.moreToReview",
                &[("count", &(candidates.len() - 1).to_string())],
            )
        }),
        explanation,
        older: card(older),
        newer: card(newer),
        outcome,
        same: translator.text("merge.sameDevice"),
        different: translator.text("merge.differentDevices"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    const NOW: i64 = 1_800_000_000_000;

    fn total(id: i64, name: &str, last_seen_ms: i64) -> ClientTotal {
        ClientTotal {
            client_id: Some(id),
            mac_address: format!("f0:18:98:00:00:{id:02}"),
            name: Some(name.into()),
            ip_address: None,
            rx_bytes: 1_000_000_000,
            tx_bytes: 0,
            since_ms: NOW - 86_400_000,
            last_seen_ms,
            months: None,
        }
    }

    fn candidate(folds: bool) -> MergeCandidate {
        MergeCandidate {
            from_key: "1".into(),
            to_key: "2".into(),
            detail: "Laptop <work>".into(),
            folds_bytes: folds,
            result_rx_bytes: 2_500_000_000,
            result_tx_bytes: 0,
        }
    }

    // SPEC-MERGE-004: the prompt names both devices and the outcome, escapes
    // the shared name, and counts what is left to review.
    #[test]
    fn the_prompt_words_the_first_proposal() {
        let english = Translator::new(Locale::En);
        let totals = [
            total(1, "Laptop <work>", NOW - 3_600_000),
            total(2, "Laptop <work>", NOW - 1_000),
        ];
        let prompt = merge_prompt(&[candidate(true), candidate(false)], &totals, NOW, &english)
            .expect("prompt");
        assert_eq!(prompt.title, "Possible duplicate device");
        assert_eq!(prompt.more.as_deref(), Some("1 more to review"));
        assert!(
            prompt.explanation.contains("<b>Laptop &lt;work&gt;</b>"),
            "{}",
            prompt.explanation
        );
        assert_eq!(prompt.older.router_id, "Router ID: 1");
        assert!(prompt.newer.detail.ends_with("Active now"));
        assert!(prompt.outcome.contains("2.5 GB"), "{}", prompt.outcome);
        assert_eq!(
            (prompt.same.as_str(), prompt.different.as_str()),
            ("Same device", "Different devices")
        );

        let apart = merge_prompt(&[candidate(false)], &totals, NOW, &english).unwrap();
        assert!(apart.outcome.starts_with("These cover different months"));
        assert_eq!(apart.more, None);
        assert_eq!(merge_prompt(&[], &totals, NOW, &english), None);
        assert_eq!(
            merge_prompt(&[candidate(true)], &totals[..1], NOW, &english),
            None
        );
    }
}
