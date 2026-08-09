//! The window: drop a notebook on it, watch it read, keep the Markdown.
//!
//! One view stack with three faces — an empty state asking for a file, a
//! reading state with a progress bar, and the transcript. Reading is not modal:
//! the transcript fills in section by section underneath the progress bar, because a
//! seventeen-thousand-pixel notebook takes minutes and a spinner for that long
//! is indistinguishable from a hang.
//!
//! The transcript view is editable. A model reading handwriting is wrong
//! sometimes, the person who wrote it can see that at a glance, and a
//! transcript you cannot correct before saving is one you have to correct
//! somewhere else.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::model::document;
use crate::model::job::{Job, Lapse, PagePlan};
use crate::model::raster::Page;
use crate::model::sections::Layout;
use crate::model::transcript::{Format, Transcript};
use crate::DEFAULT_SERVER;

use super::client::Client;
use super::runner::{Outcome, Run};
use super::tablet;

/// What the window is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Face {
    Empty,
    Reading,
    Done,
}

mod imp {
    use super::*;

    pub struct Window {
        pub client: Rc<Client>,
        pub stack: gtk::Stack,
        pub toasts: adw::ToastOverlay,

        pub status: adw::StatusPage,
        pub progress: gtk::ProgressBar,
        pub progress_label: gtk::Label,
        pub view: gtk::TextView,
        pub banner: adw::Banner,

        pub open_button: gtk::Button,
        pub stop_button: gtk::Button,
        pub save_button: gtk::Button,
        pub copy_button: gtk::Button,

        /// The run in flight, if any. Held so the Stop button can reach it.
        pub run: RefCell<Option<Rc<Run>>>,
        /// What the current transcript came from, for the save dialog's name
        /// and the frontmatter.
        pub source: RefCell<String>,
        pub model_name: RefCell<Option<String>>,
    }

    impl Default for Window {
        fn default() -> Self {
            Self {
                client: Rc::new(Client::new(DEFAULT_SERVER)),
                stack: gtk::Stack::builder()
                    .transition_type(gtk::StackTransitionType::Crossfade)
                    .build(),
                toasts: adw::ToastOverlay::new(),
                status: adw::StatusPage::builder()
                    .icon_name("document-edit-symbolic")
                    .title("Read a Notebook")
                    .description(
                        "Drop a reMarkable export here, or open one. \
                         PNG and PDF; a whole notebook at once is fine.",
                    )
                    .build(),
                progress: gtk::ProgressBar::builder().show_text(false).build(),
                progress_label: gtk::Label::builder()
                    .xalign(0.0)
                    .css_classes(["dimmed", "caption"])
                    .build(),
                view: gtk::TextView::builder()
                    .monospace(true)
                    .wrap_mode(gtk::WrapMode::WordChar)
                    .left_margin(12)
                    .right_margin(12)
                    .top_margin(12)
                    .bottom_margin(12)
                    .build(),
                banner: adw::Banner::new(""),
                open_button: gtk::Button::builder()
                    .label("Open…")
                    .action_name("win.open")
                    .build(),
                stop_button: gtk::Button::builder()
                    .icon_name("process-stop-symbolic")
                    .tooltip_text("Stop reading")
                    .action_name("win.stop")
                    .build(),
                save_button: gtk::Button::builder()
                    .icon_name("document-save-symbolic")
                    .tooltip_text("Save transcript")
                    .action_name("win.save")
                    .build(),
                copy_button: gtk::Button::builder()
                    .icon_name("edit-copy-symbolic")
                    .tooltip_text("Copy transcript")
                    .action_name("win.copy")
                    .build(),
                run: RefCell::new(None),
                source: RefCell::new(String::new()),
                model_name: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "RemarkableWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            let window = self.obj();
            window.set_title(Some("Remarkable"));
            window.set_default_size(760, 820);
            window.build();
            window.install_actions();
            window.accept_drops();
            window.probe_server();
            window.show(Face::Empty);
        }
    }

    impl WidgetImpl for Window {}
    impl WindowImpl for Window {
        /// A run left going after the window closes would keep sending
        /// requests into a view nobody can see.
        fn close_request(&self) -> glib::Propagation {
            if let Some(run) = self.run.borrow().as_ref() {
                run.cancel();
            }
            self.parent_close_request()
        }
    }
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(application: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder()
            .property("application", application)
            .build()
    }

    fn build(&self) {
        let imp = self.imp();

        let menu = gio::Menu::new();
        let sources = gio::Menu::new();
        sources.append(Some("Open File…"), Some("win.open"));
        sources.append(Some("Open From reMarkable…"), Some("win.tablet"));
        menu.append_section(None, &sources);
        menu.append(Some("About Remarkable"), Some("app.about"));

        let header = adw::HeaderBar::new();
        header.pack_start(&imp.open_button);
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Main Menu")
                .menu_model(&menu)
                .build(),
        );
        header.pack_end(&imp.save_button);
        header.pack_end(&imp.copy_button);
        header.pack_end(&imp.stop_button);

        // -- reading -------------------------------------------------------
        let reading = gtk::Box::new(gtk::Orientation::Vertical, 6);
        reading.set_margin_top(12);
        reading.set_margin_bottom(12);
        reading.set_margin_start(12);
        reading.set_margin_end(12);
        reading.append(&imp.progress_label);
        reading.append(&imp.progress);
        reading.append(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .child(&imp.view)
                .build(),
        );

        // -- done ----------------------------------------------------------
        let done = gtk::ScrolledWindow::builder().vexpand(true).build();

        imp.stack.add_named(&imp.status, Some("empty"));
        imp.stack.add_named(&reading, Some("reading"));
        imp.stack.add_named(&done, Some("done"));
        // The same text view is shown in both states; moving it rather than
        // keeping two means the text a run produced is the text that is saved.
        let _ = done;

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&imp.banner);
        content.append(&imp.stack);
        imp.stack.set_vexpand(true);

        imp.toasts.set_child(Some(&content));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&imp.toasts));
        self.set_content(Some(&toolbar));

        let open = gtk::Button::builder()
            .label("Open a Notebook")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .action_name("win.open")
            .build();
        imp.status.set_child(Some(&open));
    }

    /// Enable or disable one of this window's actions.
    ///
    /// `WidgetExt::action_set_enabled` does not reach these: it drives actions
    /// installed on the widget *class*, and these were added to the window's
    /// `ActionMap` as entries. Calling it here silently did nothing, and Save
    /// and Copy stayed live on an empty window.
    fn offer(&self, name: &str, enabled: bool) {
        if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            action.set_enabled(enabled);
        }
    }

    fn install_actions(&self) {
        let open = gio::ActionEntry::builder("open")
            .activate(|window: &Self, _, _| window.choose_file())
            .build();
        let save = gio::ActionEntry::builder("save")
            .activate(|window: &Self, _, _| window.save())
            .build();
        let copy = gio::ActionEntry::builder("copy")
            .activate(|window: &Self, _, _| window.copy())
            .build();
        let stop = gio::ActionEntry::builder("stop")
            .activate(|window: &Self, _, _| window.stop())
            .build();
        let from_tablet = gio::ActionEntry::builder("tablet")
            .activate(|window: &Self, _, _| window.choose_from_tablet())
            .build();
        self.add_action_entries([open, save, copy, stop, from_tablet]);
    }

    /// Files dropped anywhere on the window.
    fn accept_drops(&self) {
        let target = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        target.connect_drop(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Ok(file) = value.get::<gio::File>() else {
                    return false;
                };
                window.open(&file);
                true
            }
        ));
        self.add_controller(target);
    }

    fn probe_server(&self) {
        self.imp().client.probe(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |result| match result {
                Ok(info) => {
                    window.imp().model_name.replace(info.model.clone());
                    if !info.vision {
                        window.warn(
                            "The model on llama-server cannot see images. \
                             Load one with a vision projector to read handwriting.",
                        );
                    } else {
                        window.imp().banner.set_revealed(false);
                    }
                }
                Err(error) => window.warn(&format!(
                    "llama-server is not answering at {} — {error}",
                    window.imp().client.base_url()
                )),
            }
        ));
    }

    fn warn(&self, message: &str) {
        let banner = &self.imp().banner;
        banner.set_title(message);
        banner.set_revealed(true);
    }

    fn choose_file(&self) {
        let notebooks = gtk::FileFilter::new();
        notebooks.set_name(Some("Notebooks and images"));
        notebooks.add_mime_type("application/pdf");
        notebooks.add_mime_type("image/png");
        notebooks.add_mime_type("image/jpeg");

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&notebooks);

        gtk::FileDialog::builder()
            .title("Open a Notebook")
            .filters(&filters)
            .build()
            .open(
                Some(self),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |result| {
                        if let Ok(file) = result {
                            window.open(&file);
                        }
                    }
                ),
            );
    }

    /// List what is on the tablet and read one of them.
    fn choose_from_tablet(&self) {
        if self.imp().run.borrow().is_some() {
            self.toast("Already reading a notebook.");
            return;
        }
        tablet::present(
            self,
            Rc::clone(&self.imp().client),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |name: String, pdf: Vec<u8>| window.read_bytes(&name, &pdf)
            ),
        );
    }

    /// Read a file. The public entry point — the drop target, the file dialog
    /// and `open` on the application all arrive here.
    pub fn open(&self, file: &gio::File) {
        if self.imp().run.borrow().is_some() {
            self.toast("Already reading a notebook.");
            return;
        }

        let name = file
            .basename()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "notebook".into());

        match std::fs::read(file.path().unwrap_or_default()) {
            Ok(bytes) => self.read_bytes(&name, &bytes),
            Err(error) => self.toast(&format!("Could not read {name} — {error}")),
        }
    }

    /// Rasterise a document and start reading it. Both a file on disk and a
    /// PDF the tablet just rendered arrive here.
    fn read_bytes(&self, name: &str, bytes: &[u8]) {
        let pages = if document::is_pdf(bytes) {
            match document::rasterise(bytes) {
                Ok(pages) => pages,
                Err(problem) => {
                    self.toast(&problem);
                    return;
                }
            }
        } else {
            match Page::decode(bytes) {
                Ok(page) => vec![page],
                Err(error) => {
                    self.toast(&format!("{name}: {error}"));
                    return;
                }
            }
        };

        if pages.is_empty() {
            self.toast(&format!("{name} has no pages."));
            return;
        }

        self.imp().source.replace(name.to_string());
        self.start(pages);
    }

    fn start(&self, pages: Vec<Page>) {
        let imp = self.imp();

        let layout = Layout::default();
        let plans: Vec<PagePlan> = pages
            .iter()
            .enumerate()
            .map(|(index, page)| {
                PagePlan::from_profile(index + 1, &page.profile(), page.width(), &layout)
            })
            .collect();

        let job = Job::new(plans);
        self.set_text("");
        self.show(Face::Reading);
        self.report(&job);

        let run = Run::start(
            Rc::clone(&imp.client),
            pages,
            job,
            imp.model_name.borrow().clone(),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |job: &Job| {
                    window.set_text(&job.text_so_far());
                    window.report(job);
                }
            ),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |job: &Job, outcome: Outcome| window.finished(job, outcome)
            ),
        );

        imp.run.replace(Some(run));
    }

    fn report(&self, job: &crate::model::job::Job) {
        let imp = self.imp();
        let progress = job.progress();
        imp.progress.set_fraction(progress.fraction());
        imp.progress_label.set_label(&if progress.pages > 1 {
            format!(
                "Reading page {} of {} — section {} of {}",
                progress.page,
                progress.pages,
                (progress.done + 1).min(progress.total),
                progress.total
            )
        } else {
            format!(
                "Reading section {} of {}",
                (progress.done + 1).min(progress.total),
                progress.total
            )
        });
    }

    fn finished(&self, job: &Job, outcome: Outcome) {
        let imp = self.imp();
        imp.run.replace(None);
        self.set_text(&job.text_so_far());
        self.show(Face::Done);

        let failures = job
            .lapses()
            .iter()
            .filter(|(_, lapse)| matches!(lapse, Lapse::Failed(_)))
            .count();

        let message = match (outcome, failures) {
            (Outcome::Cancelled, _) => "Stopped. What was read so far is here.".to_string(),
            (_, 0) => format!("Read {} sections.", job.total_sections()),
            (_, 1) => "One section could not be read; the rest is here.".to_string(),
            (_, n) => format!("{n} sections could not be read; the rest is here."),
        };
        self.toast(&message);
    }

    fn stop(&self) {
        if let Some(run) = self.imp().run.borrow().as_ref() {
            run.cancel();
        }
    }

    fn transcript(&self) -> Transcript {
        let imp = self.imp();
        let buffer = imp.view.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();

        Transcript {
            source: imp.source.borrow().clone(),
            model: imp
                .model_name
                .borrow()
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            processed: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            pages: vec![crate::model::transcript::PageText {
                number: 1,
                text,
                sections: 1,
            }],
        }
    }

    fn copy(&self) {
        let transcript = self.transcript();
        self.clipboard().set_text(&transcript.body());
        self.toast("Transcript copied.");
    }

    fn save(&self) {
        let transcript = self.transcript();
        let stem = std::path::Path::new(&*self.imp().source.borrow())
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "transcript".into());

        gtk::FileDialog::builder()
            .title("Save Transcript")
            .initial_name(Transcript::filename(&stem, Format::Markdown))
            .build()
            .save(
                Some(self),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |result| {
                        let Ok(file) = result else { return };
                        let Some(path) = file.path() else { return };
                        let format = match path.extension().and_then(|e| e.to_str()) {
                            Some("json") => Format::Json,
                            Some("txt") => Format::Text,
                            _ => Format::Markdown,
                        };
                        match std::fs::write(&path, transcript.render(format)) {
                            Ok(()) => window.toast("Transcript saved."),
                            Err(error) => window.toast(&format!("Could not save — {error}")),
                        }
                    }
                ),
            );
    }

    fn set_text(&self, text: &str) {
        self.imp().view.buffer().set_text(text);
    }

    fn toast(&self, message: &str) {
        self.imp().toasts.add_toast(adw::Toast::new(message));
    }

    fn show(&self, face: Face) {
        let imp = self.imp();
        imp.stack.set_visible_child_name(match face {
            Face::Empty => "empty",
            // Reading and Done share the text view, so both show the same page
            // and only the progress bar's visibility differs.
            Face::Reading | Face::Done => "reading",
        });
        imp.progress.set_visible(face == Face::Reading);
        imp.progress_label.set_visible(face == Face::Reading);
        imp.stop_button.set_visible(face == Face::Reading);
        imp.save_button.set_visible(face == Face::Done);
        imp.copy_button.set_visible(face == Face::Done);
        imp.view.set_editable(face == Face::Done);

        self.offer("stop", face == Face::Reading);
        self.offer("save", face == Face::Done);
        self.offer("copy", face == Face::Done);
        self.offer("open", face != Face::Reading);
        self.offer("tablet", face != Face::Reading);
    }
}
