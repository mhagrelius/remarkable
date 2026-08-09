use gtk::prelude::*;
use remarkable::ui::RemarkableApplication;

fn main() -> gtk::glib::ExitCode {
    gtk::glib::set_application_name("Remarkable");
    gtk::glib::set_prgname(Some(remarkable::APP_ID));
    RemarkableApplication::new().run()
}
