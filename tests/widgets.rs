//! The widget tree, built and driven with no server and no window on screen.
//!
//! One `#[test]`, containing a list of cases. GTK is thread-affine and
//! `--test-threads=1` only serialises tests — it does not make them share a
//! thread — so a second `#[test]` touching a widget is a second thread calling
//! into GTK, which is undefined rather than slow.
//!
//! Windows are constructed and never presented. Nothing here reaches the
//! network: the window probes llama-server on construction, and because that is
//! an async call on the main loop that this test never runs, the probe is
//! issued and never completes. That is the point — it proves the window is
//! usable before the server answers.

use adw::prelude::*;
use gio::prelude::ActionGroupExt;
use gtk::gio;

use remarkable::ui::{RemarkableApplication, Window};

type Case = (&'static str, fn());

const CASES: &[Case] = &[
    ("a fresh window offers the things you can do", fresh_window),
    (
        "nothing that needs a transcript is offered yet",
        nothing_to_save,
    ),
    ("the window survives a file that is not one", rubbish_input),
];

#[test]
fn widgets() {
    let application = RemarkableApplication::new();
    // `register` runs `startup`, which is where the actions are installed;
    // `run` would block on a main loop this test does not want.
    application
        .register(gio::Cancellable::NONE)
        .expect("registers");

    for (name, case) in CASES {
        eprintln!("  {name}");
        case();
    }
}

fn window() -> Window {
    let application = adw::Application::builder()
        .application_id("us.hagreli.Remarkable.Test")
        .build();
    Window::new(&application)
}

fn enabled(window: &Window, action: &str) -> bool {
    ActionGroupExt::is_action_enabled(window, action)
}

fn fresh_window() {
    let window = window();
    assert_eq!(window.title().as_deref(), Some("Remarkable"));

    // Opening something is the only thing there is to do on an empty window,
    // and both roads to it must be live before the server has answered.
    assert!(enabled(&window, "open"), "Open is not offered");
    assert!(
        enabled(&window, "tablet"),
        "the tablet picker is not offered"
    );
}

fn nothing_to_save() {
    let window = window();
    for action in ["save", "copy", "stop"] {
        assert!(
            !enabled(&window, action),
            "{action} is offered with no transcript to act on"
        );
    }
}

fn rubbish_input() {
    let window = window();

    let directory = std::env::temp_dir().join("remarkable-widget-test");
    std::fs::create_dir_all(&directory).expect("a temp dir");
    let path = directory.join("not-an-image.txt");
    std::fs::write(&path, b"this is a text file, not a notebook").expect("writes");

    // A dropped text file must complain and leave the window alone, not panic
    // and not start a run.
    window.open(&gio::File::for_path(&path));

    assert!(
        enabled(&window, "open"),
        "the window locked up on bad input"
    );
    assert!(!enabled(&window, "stop"), "a run started on a text file");

    let _ = std::fs::remove_dir_all(&directory);
}
