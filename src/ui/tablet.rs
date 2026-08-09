//! The dialog that lists what is on the tablet and fetches one.
//!
//! Everything it knows about the protocol comes from [`crate::model::device`];
//! this file is the widgets and the two HTTP calls. The listing is fetched
//! recursively — the tablet answers one folder at a time — and flattened,
//! because a notebook is chosen by its name and not by walking a tree.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::model::device::{self, Item, Kind, Source};

use super::client::Client;

/// Show the picker. `on_chosen` is handed the notebook's name and the PDF the
/// tablet rendered for it.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    client: Rc<Client>,
    on_chosen: impl Fn(String, Vec<u8>) + 'static,
) {
    // One handle, shared by every row's handler.
    let on_chosen: Chosen = Rc::new(on_chosen);

    let dialog = adw::Dialog::builder()
        .title("Open From reMarkable")
        .content_width(460)
        .content_height(560)
        .build();

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();

    let status = adw::StatusPage::builder()
        .icon_name("drive-harddisk-usb-symbolic")
        .title("Looking for a Tablet")
        .description(format!(
            "Plug the tablet in and turn on Settings → Storage → USB web \
             interface, then it answers at {}.",
            device::USB_HOST
        ))
        .build();

    let stack = gtk::Stack::new();
    stack.add_named(
        &adw::Spinner::builder()
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build(),
        Some("looking"),
    );
    stack.add_named(&status, Some("absent"));
    let padded = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    padded.append(&list);
    stack.add_named(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&padded)
            .build(),
        Some("library"),
    );

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));

    stack.set_visible_child_name("looking");

    // The tablet answers a folder at a time, so the walk is a queue of folders
    // still to ask about and a list of everything seen so far.
    let found: Rc<RefCell<Vec<Item>>> = Rc::new(RefCell::new(Vec::new()));
    walk(
        Source::Usb,
        Rc::clone(&client),
        vec![None],
        Rc::clone(&found),
        glib::clone!(
            #[strong]
            client,
            #[strong]
            stack,
            #[strong]
            list,
            #[strong]
            dialog,
            #[strong]
            on_chosen,
            move |outcome: Result<Vec<Item>, String>| {
                let items = match outcome {
                    Ok(items) => items,
                    Err(problem) => {
                        status.set_description(Some(&problem));
                        stack.set_visible_child_name("absent");
                        return;
                    }
                };

                let mut notebooks: Vec<Item> = items
                    .into_iter()
                    .filter(|item| item.kind == Kind::Document && !item.is_deleted())
                    .collect();
                // Handwriting first, then anything annotated, each A-Z: the app
                // is for the first kind and the list should say so.
                notebooks.sort_by(|a, b| {
                    b.is_notebook()
                        .cmp(&a.is_notebook())
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });

                if notebooks.is_empty() {
                    status.set_title("Nothing on the Tablet");
                    status.set_description(Some("The tablet answered, with an empty library."));
                    stack.set_visible_child_name("absent");
                    return;
                }

                for item in notebooks {
                    let row = adw::ActionRow::builder()
                        .title(glib::markup_escape_text(&item.name))
                        .subtitle(if item.is_notebook() {
                            "Handwriting"
                        } else {
                            "Annotated document"
                        })
                        .activatable(true)
                        .build();
                    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

                    row.connect_activated(glib::clone!(
                        #[strong]
                        client,
                        #[strong]
                        dialog,
                        #[strong]
                        item,
                        #[strong]
                        on_chosen,
                        move |row| {
                            row.set_sensitive(false);
                            fetch(
                                &client,
                                &item,
                                dialog.clone(),
                                row.clone(),
                                Rc::clone(&on_chosen),
                            );
                        }
                    ));
                    list.append(&row);
                }
                stack.set_visible_child_name("library");
            }
        ),
    );
}

/// Ask for a folder, then for every folder it contained, until none are left.
fn walk(
    source: Source,
    client: Rc<Client>,
    mut queue: Vec<Option<String>>,
    found: Rc<RefCell<Vec<Item>>>,
    done: impl Fn(Result<Vec<Item>, String>) + 'static,
) {
    let Some(folder) = queue.pop() else {
        let items = found.borrow().clone();
        done(Ok(items));
        return;
    };

    let url = match source.listing_url(folder.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            done(Err(error.to_string()));
            return;
        }
    };

    let at_root = folder.is_none();
    let carried = Rc::clone(&client);
    carried.fetch(&url, move |result| {
        let body = match result {
            Ok(body) => body,
            Err(error) => {
                // A folder that fails part-way through is skipped; only the
                // root failing means there is no tablet to talk to.
                if at_root {
                    done(Err(format!("The tablet did not answer — {error}")));
                } else {
                    walk(source, client, queue, found, done);
                }
                return;
            }
        };

        match device::parse_listing(&String::from_utf8_lossy(&body)) {
            Ok(items) => {
                for item in &items {
                    if item.kind == Kind::Folder && !item.is_deleted() {
                        queue.push(Some(item.id.clone()));
                    }
                }
                found.borrow_mut().extend(items);
                walk(source, client, queue, found, done);
            }
            Err(error) if at_root => done(Err(error.to_string())),
            Err(_) => walk(source, client, queue, found, done),
        }
    });
}

fn fetch(
    client: &Rc<Client>,
    item: &Item,
    dialog: adw::Dialog,
    row: adw::ActionRow,
    on_chosen: Chosen,
) {
    let url = match Source::Usb.pdf_url(&item.id) {
        Ok(url) => url,
        Err(error) => {
            row.set_subtitle(&error.to_string());
            row.set_sensitive(true);
            return;
        }
    };

    // Rendering a long notebook on the tablet takes it a while, and a row that
    // does nothing for thirty seconds looks broken.
    row.set_subtitle("Asking the tablet to render it…");
    let name = device::safe_filename(&item.name);

    client.fetch(&url, move |result| match result {
        Ok(pdf) => {
            dialog.close();
            on_chosen(format!("{name}.pdf"), pdf);
        }
        Err(error) => {
            row.set_subtitle(&format!("Could not fetch it — {error}"));
            row.set_sensitive(true);
        }
    });
}

/// What to do with the notebook the person picked. Shared, because every row
/// in the list needs a handle to the one callback the caller passed in.
type Chosen = Rc<dyn Fn(String, Vec<u8>)>;
