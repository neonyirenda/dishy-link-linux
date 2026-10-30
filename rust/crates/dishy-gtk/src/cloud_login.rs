//! Isolated WebKit login surface for the Starlink account.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use webkit6::prelude::*;

type CookieCallback = Rc<dyn Fn(String)>;

/// The sign-in browser's chrome is separate so cancellation can be exercised
/// without navigating WebKit or contacting the real Starlink site.
pub fn build_sign_in_window(
    parent: &adw::ApplicationWindow,
    title: &str,
    close_label: &str,
    content: &impl IsA<gtk::Widget>,
) -> (adw::Window, gtk::Button) {
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("title-3");
    heading.set_hexpand(true);
    heading.set_xalign(0.5);
    let close = gtk::Button::with_label("×");
    close.set_widget_name("starlink-login-close");
    close.set_tooltip_text(Some(close_label));
    close.add_css_class("flat");
    close.add_css_class("circular");
    close.set_size_request(36, 36);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(8);
    toolbar.set_margin_bottom(8);
    toolbar.set_margin_start(12);
    toolbar.set_margin_end(12);
    toolbar.append(&heading);
    toolbar.append(&close);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&toolbar);
    layout.append(content);
    content.set_vexpand(true);
    content.set_hexpand(true);

    let window = adw::Window::builder()
        .title(title)
        .default_width(980)
        .default_height(720)
        .transient_for(parent)
        .modal(true)
        .content(&layout)
        .build();
    let weak_window = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak_window.upgrade() {
            window.close();
        }
    });
    (window, close)
}

pub fn show_sign_in(
    parent: &adw::ApplicationWindow,
    title: &str,
    close_label: &str,
    on_cookie: impl Fn(String) + 'static,
) -> adw::Window {
    // Login cookies stay ephemeral here. Only the validated session copied to
    // Secret Service survives the window; WebKit must not leave a second,
    // plaintext-capable credential store behind.
    let session = webkit6::NetworkSession::new_ephemeral();
    // Authentication is captured from the cookie jar and persisted only after
    // host verification. Do not ask WebKit's separate password store to write
    // credentials through the desktop keyring socket during this flow.
    session.set_persistent_credential_storage_enabled(false);
    let Some(cookie_manager) = session.cookie_manager() else {
        tracing::error!("WebKit login session has no cookie manager");
        let message = gtk::Label::new(Some("Could not start the sign-in browser."));
        let (window, _) = build_sign_in_window(parent, title, close_label, &message);
        window.present();
        return window;
    };
    let webview = webkit6::WebView::builder()
        .network_session(&session)
        .build();
    let (window, _close) = build_sign_in_window(parent, title, close_label, &webview);

    let last_sent = Rc::new(RefCell::new(None));
    let on_cookie: CookieCallback = Rc::new(on_cookie);
    let last_sent_for_load = last_sent.clone();
    let callback_for_load = on_cookie.clone();
    let manager_for_load = cookie_manager.clone();
    webview.connect_load_changed(move |_, _| {
        try_adopt_session(
            manager_for_load.clone(),
            last_sent_for_load.clone(),
            callback_for_load.clone(),
        );
    });
    // Starlink's account UI can finish authentication in-place without another
    // navigation, so `load_changed` alone misses the successful login. Watch
    // the ephemeral cookie jar too. The host verifies each new cookie snapshot;
    // only its successful account response closes this modal.
    let last_sent_for_cookie = last_sent.clone();
    let callback_for_cookie = on_cookie.clone();
    cookie_manager.connect_changed(move |manager| {
        try_adopt_session(
            manager.clone(),
            last_sent_for_cookie.clone(),
            callback_for_cookie.clone(),
        );
    });
    webview.load_uri("https://www.starlink.com/account");
    window.present();
    window
}

fn try_adopt_session(
    manager: webkit6::CookieManager,
    last_sent: Rc<RefCell<Option<String>>>,
    on_cookie: CookieCallback,
) {
    gtk::glib::MainContext::default().spawn_local(async move {
        // The whole jar, not the cookies one page URL would send: the session
        // includes cookies scoped to other starlink.com hosts and paths.
        let cookies = match manager.all_cookies_future().await {
            Ok(cookies) => cookies,
            Err(error) => {
                tracing::warn!("sign-in: could not read the login cookie jar: {error}");
                return;
            }
        };
        let jar: Vec<(String, String, String)> = cookies
            .into_iter()
            .filter_map(|mut cookie| {
                let domain = cookie
                    .domain()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                Some((
                    domain,
                    cookie.name()?.to_string(),
                    cookie.value()?.to_string(),
                ))
            })
            .collect();
        let Some(cookie) = session_header(&jar) else {
            return;
        };
        let mut last = last_sent.borrow_mut();
        if last.as_deref() != Some(cookie.as_str()) {
            // Names only: values are the credential.
            let names: Vec<&str> = cookie
                .split("; ")
                .filter_map(|pair| pair.split('=').next())
                .collect();
            tracing::info!(
                "sign-in: submitting session for verification ({})",
                names.join(", ")
            );
            *last = Some(cookie.clone());
            on_cookie(cookie);
        }
    });
}

/// The candidate session header from the whole login jar (spec 0140): every
/// `starlink.com` cookie, whatever host or path it is scoped to, or `None`
/// until the SSO cookie that marks a finished login is there.
pub fn session_header(cookies: &[(String, String, String)]) -> Option<String> {
    let ours: Vec<(&str, &str)> = cookies
        .iter()
        .filter(|(domain, _, _)| {
            let domain = domain.trim_start_matches('.');
            domain == "starlink.com" || domain.ends_with(".starlink.com")
        })
        .map(|(_, name, value)| (name.as_str(), value.as_str()))
        .collect();
    if !ours.iter().any(|(name, _)| *name == "Starlink.Com.Sso") {
        return None;
    }
    Some(
        ours.iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str, name: &str, value: &str) -> (String, String, String) {
        (domain.into(), name.into(), value.into())
    }

    // SPEC-CLOUD-010: the header carries every starlink.com cookie from any
    // subdomain, leaves other sites out, and waits for the SSO cookie.
    #[test]
    fn the_session_is_the_whole_starlink_jar() {
        let jar = vec![
            cookie(".starlink.com", "Starlink.Com.Sso", "s"),
            cookie("api.starlink.com", "Access.V1", "a"),
            cookie("www.starlink.com", "XSRF-TOKEN", "x"),
            cookie("starlink.com", "locale", "en"),
            cookie(".notstarlink.com", "tracker", "t"),
            cookie(".google.com", "NID", "g"),
        ];
        assert_eq!(
            session_header(&jar).as_deref(),
            Some("Starlink.Com.Sso=s; Access.V1=a; XSRF-TOKEN=x; locale=en")
        );
        assert_eq!(session_header(&jar[1..]), None, "no SSO cookie yet");
        assert_eq!(session_header(&[]), None);
    }
}
