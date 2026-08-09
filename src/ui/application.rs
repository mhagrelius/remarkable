//! The application object: actions, accelerators, and the one window.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::APP_ID;

use super::window::Window;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RemarkableApplication;

    #[glib::object_subclass]
    impl ObjectSubclass for RemarkableApplication {
        const NAME: &'static str = "RemarkableApplication";
        type Type = super::RemarkableApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for RemarkableApplication {}

    impl ApplicationImpl for RemarkableApplication {
        fn startup(&self) {
            // Chain up first: the toolkit initialises in the parent's handler,
            // and anything built before it returns is built against nothing.
            self.parent_startup();
            self.obj().install_actions();
        }

        fn activate(&self) {
            let application = self.obj();
            let window = application
                .active_window()
                .unwrap_or_else(|| Window::new(&*application).upcast());
            window.present();
        }

        /// Opened with files — from the file manager, or a second launch with
        /// arguments. Each becomes a document to read.
        fn open(&self, files: &[gio::File], _hint: &str) {
            let application = self.obj();
            let window = match application.active_window().and_downcast::<Window>() {
                Some(window) => window,
                None => Window::new(&*application),
            };
            window.present();
            if let Some(file) = files.first() {
                window.open(file);
            }
        }
    }

    impl GtkApplicationImpl for RemarkableApplication {}
    impl AdwApplicationImpl for RemarkableApplication {}
}

glib::wrapper! {
    pub struct RemarkableApplication(ObjectSubclass<imp::RemarkableApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for RemarkableApplication {
    fn default() -> Self {
        Self::new()
    }
}

impl RemarkableApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .build()
    }

    fn install_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|application: &Self, _, _| application.quit())
            .build();

        let about = gio::ActionEntry::builder("about")
            .activate(|application: &Self, _, _| application.show_about())
            .build();

        self.add_action_entries([quit, about]);
        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("win.open", &["<Control>o"]);
        self.set_accels_for_action("win.save", &["<Control>s"]);
        self.set_accels_for_action("win.copy", &["<Control><Shift>c"]);
        self.set_accels_for_action("win.stop", &["Escape"]);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Remarkable")
            .application_icon(APP_ID)
            .developer_name("Matthew Hagrelius")
            .version(env!("CARGO_PKG_VERSION"))
            .license_type(gtk::License::Gpl30)
            .comments(
                "Handwritten notes off a reMarkable tablet, read by a model on this \
                 machine and written out as Markdown. Nothing leaves the device.",
            )
            .build();
        about.present(self.active_window().as_ref());
    }
}
