//! Getting notebooks off the tablet, without paying for the cloud.
//!
//! A tablet in developer mode serves its own library over HTTP on the USB
//! network — `http://10.11.99.1`, once *Settings → Storage → USB web interface*
//! is on. That interface is the transfer mechanism here, for one reason that
//! decides everything else: `GET /download/{id}/placeholder` returns a PDF the
//! **tablet itself rendered**. Its own strokes, its own fonts, its own layout.
//!
//! The alternative is to copy `~/.local/share/remarkable/xochitl` over SSH and
//! parse the `.rm` v6 line format. That is a stroke-geometry parser and a
//! renderer — thousands of lines, versioned against firmware — to arrive at a
//! worse picture than the device will hand over for free. So it is not done.
//!
//! SSH still earns its place, because the web interface binds to the USB
//! network and nothing else: unplugged, `10.11.99.1` does not answer. What SSH
//! provides is a way to reach that same interface from anywhere the tablet is
//! on the network, by forwarding a local port to port 80 on the device. Same
//! HTTP client, same endpoints, different address. [`Source::tunnel`] builds
//! the address and [`tunnel_command`] the `ssh` invocation that opens it.
//!
//! Nothing in this module performs I/O. It builds URLs and argument vectors and
//! reads the JSON that comes back, so all of it is tested with the tablet in a
//! drawer.

use serde::Deserialize;

/// The tablet's address on the USB network. Fixed in firmware.
pub const USB_HOST: &str = "10.11.99.1";

/// Where the library is being read from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Source {
    /// Plugged in, over the USB network.
    #[default]
    Usb,
    /// Through a forwarded local port, for a tablet that is on the network but
    /// not on the end of a cable.
    Tunnel { port: u16 },
}

impl Source {
    pub fn tunnel(port: u16) -> Self {
        Self::Tunnel { port }
    }

    /// The origin every endpoint hangs off.
    pub fn base_url(&self) -> String {
        match self {
            Self::Usb => format!("http://{USB_HOST}"),
            // Explicitly loopback rather than `localhost`, which resolves to
            // ::1 first on this system while ssh forwards 127.0.0.1.
            Self::Tunnel { port } => format!("http://127.0.0.1:{port}"),
        }
    }

    /// The contents of a folder, or of the root when `folder` is `None`.
    pub fn listing_url(&self, folder: Option<&str>) -> Result<String, DeviceError> {
        Ok(match folder {
            None => format!("{}/documents/", self.base_url()),
            Some(id) => format!("{}/documents/{}", self.base_url(), checked_id(id)?),
        })
    }

    /// The tablet's own rendering of a document, as a PDF.
    ///
    /// `placeholder` rather than `pdf`: both are documented, but `placeholder`
    /// is the path that has worked across every firmware anyone has written
    /// down, and the name is a leftover rather than a description.
    pub fn pdf_url(&self, id: &str) -> Result<String, DeviceError> {
        Ok(format!(
            "{}/download/{}/placeholder",
            self.base_url(),
            checked_id(id)?
        ))
    }

    /// The page thumbnail, for the picker to show something other than a list
    /// of names.
    pub fn thumbnail_url(&self, id: &str) -> Result<String, DeviceError> {
        Ok(format!("{}/thumbnail/{}", self.base_url(), checked_id(id)?))
    }
}

/// The `ssh` that opens the tunnel [`Source::Tunnel`] talks through.
///
/// Returned as an argument vector, never a shell string: `host` comes from a
/// text entry and a hostname with a semicolon in it must not become a second
/// command.
///
/// `-N` because no remote command is wanted, only the forward.
pub fn tunnel_command(host: &str, user: &str, port: u16) -> Vec<String> {
    vec![
        "ssh".into(),
        "-N".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        // The tablet regenerates its host key on some firmware updates, and a
        // changed key should be a prompt, not a silent connection.
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-L".into(),
        format!("127.0.0.1:{port}:127.0.0.1:80"),
        format!("{user}@{host}"),
    ]
}

/// The default account on a reMarkable. The password is under *Settings → Help
/// → About → Copyrights and licenses*, at the bottom of the GPLv3 notice.
pub const SSH_USER: &str = "root";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// An identifier that is not one, and must not be pasted into a URL.
    BadIdentifier(String),
    /// The tablet answered with something that is not a listing.
    Unreadable(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadIdentifier(id) => write!(f, "{id} is not a document identifier"),
            Self::Unreadable(detail) => {
                write!(f, "the tablet's answer could not be read ({detail})")
            }
        }
    }
}

/// Whether something is a document or a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Document,
    Folder,
}

/// One entry in the tablet's library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The folder this sits in. Empty at the root; `"trash"` when deleted.
    pub parent: String,
    /// `"notebook"` for handwriting, `"pdf"` or `"epub"` for something
    /// imported and annotated.
    pub file_type: String,
}

impl Item {
    /// Whether this is handwriting the tablet drew, rather than a document
    /// someone loaded onto it. Both transcribe; only the first is what this app
    /// is for, and the picker leads with them.
    pub fn is_notebook(&self) -> bool {
        self.kind == Kind::Document && (self.file_type.is_empty() || self.file_type == "notebook")
    }

    pub fn is_deleted(&self) -> bool {
        self.parent == "trash"
    }
}

/// The wire shape. Field names are the tablet's, including the misspelling:
/// firmware has served `VissibleName` since the first release and some builds
/// serve `VisibleName` beside it, so both are read.
#[derive(Debug, Deserialize)]
struct WireItem {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "VissibleName", default)]
    vissible_name: Option<String>,
    #[serde(rename = "VisibleName", default)]
    visible_name: Option<String>,
    #[serde(rename = "Type", default)]
    kind: String,
    #[serde(rename = "Parent", default)]
    parent: String,
    #[serde(rename = "fileType", default)]
    file_type: String,
}

/// Read a `/documents/` listing.
///
/// An entry with no identifier is dropped rather than failing the listing: one
/// unreadable notebook should not hide the other forty.
pub fn parse_listing(body: &str) -> Result<Vec<Item>, DeviceError> {
    let items: Vec<WireItem> =
        serde_json::from_str(body).map_err(|error| DeviceError::Unreadable(error.to_string()))?;

    Ok(items
        .into_iter()
        .filter(|item| !item.id.is_empty())
        .map(|item| {
            let name = item
                .vissible_name
                .or(item.visible_name)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Untitled".to_string());
            Item {
                id: item.id,
                name,
                kind: match item.kind.as_str() {
                    "CollectionType" => Kind::Folder,
                    _ => Kind::Document,
                },
                parent: item.parent,
                file_type: item.file_type,
            }
        })
        .collect())
}

/// A name that is safe to save a downloaded notebook under.
///
/// Notebook titles are free text and routinely contain `/`. This is what stands
/// between "Q1 / planning" and a write into the parent directory.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "Untitled".into()
    } else {
        cleaned
    }
}

/// A document identifier that can only ever be one path segment.
///
/// The identifiers are UUIDs, but they arrive from the tablet's own JSON and go
/// straight into a URL, so they are checked rather than trusted.
fn checked_id(id: &str) -> Result<&str, DeviceError> {
    let plausible = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if plausible {
        Ok(id)
    } else {
        Err(DeviceError::BadIdentifier(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "c6c15720-0210-4459-96f0-1963ee001685";

    #[test]
    fn a_plugged_in_tablet_is_addressed_on_the_usb_network() {
        let usb = Source::Usb;
        assert_eq!(
            usb.listing_url(None).expect("a url"),
            "http://10.11.99.1/documents/"
        );
        assert_eq!(
            usb.listing_url(Some(GUID)).expect("a url"),
            format!("http://10.11.99.1/documents/{GUID}")
        );
        assert_eq!(
            usb.pdf_url(GUID).expect("a url"),
            format!("http://10.11.99.1/download/{GUID}/placeholder")
        );
        assert_eq!(
            usb.thumbnail_url(GUID).expect("a url"),
            format!("http://10.11.99.1/thumbnail/{GUID}")
        );
    }

    #[test]
    fn a_tunnelled_tablet_is_the_same_endpoints_at_a_local_port() {
        let tunnel = Source::tunnel(8081);
        assert_eq!(
            tunnel.listing_url(None).expect("a url"),
            "http://127.0.0.1:8081/documents/"
        );
        assert_eq!(
            tunnel.pdf_url(GUID).expect("a url"),
            format!("http://127.0.0.1:8081/download/{GUID}/placeholder")
        );
    }

    #[test]
    fn an_identifier_cannot_climb_out_of_its_path_segment() {
        for id in [
            "../../etc/passwd",
            "a/b",
            "",
            "guid?x=1",
            "guid#frag",
            "guid with spaces",
        ] {
            assert!(
                matches!(Source::Usb.pdf_url(id), Err(DeviceError::BadIdentifier(_))),
                "{id} was accepted"
            );
            assert!(
                Source::Usb.listing_url(Some(id)).is_err(),
                "{id} was accepted"
            );
        }
    }

    #[test]
    fn the_tunnel_is_an_argument_vector_not_a_shell_string() {
        let command = tunnel_command("remarkable.local", SSH_USER, 8081);
        assert_eq!(command[0], "ssh");
        assert!(command.contains(&"-N".to_string()));
        assert!(command.contains(&"127.0.0.1:8081:127.0.0.1:80".to_string()));
        assert_eq!(command.last().expect("a host"), "root@remarkable.local");
    }

    #[test]
    fn a_hostname_with_a_semicolon_stays_one_argument() {
        let command = tunnel_command("host; rm -rf ~", SSH_USER, 8081);
        assert_eq!(command.last().expect("a host"), "root@host; rm -rf ~");
        assert!(!command.iter().any(|arg| arg == "rm"));
    }

    #[test]
    fn a_listing_is_read_into_documents_and_folders() {
        let body = r#"[
            {"ID":"aaa","VissibleName":"Rust Book Notes","Type":"DocumentType",
             "Parent":"","fileType":"notebook"},
            {"ID":"bbb","VissibleName":"Work","Type":"CollectionType","Parent":""},
            {"ID":"ccc","VissibleName":"Contract","Type":"DocumentType",
             "Parent":"bbb","fileType":"pdf"}
        ]"#;
        let items = parse_listing(body).expect("a listing");
        assert_eq!(items.len(), 3);

        assert_eq!(items[0].name, "Rust Book Notes");
        assert_eq!(items[0].kind, Kind::Document);
        assert!(items[0].is_notebook());

        assert_eq!(items[1].kind, Kind::Folder);
        assert!(!items[1].is_notebook());

        assert_eq!(items[2].parent, "bbb");
        // An annotated PDF is a document, but not handwriting the tablet drew.
        assert!(!items[2].is_notebook());
    }

    #[test]
    fn the_firmwares_two_spellings_of_the_name_field_are_both_read() {
        let misspelled = r#"[{"ID":"a","VissibleName":"Notes","Type":"DocumentType"}]"#;
        let corrected = r#"[{"ID":"a","VisibleName":"Notes","Type":"DocumentType"}]"#;
        assert_eq!(
            parse_listing(misspelled).expect("a listing")[0].name,
            "Notes"
        );
        assert_eq!(
            parse_listing(corrected).expect("a listing")[0].name,
            "Notes"
        );
    }

    #[test]
    fn a_notebook_with_no_name_is_still_listed() {
        let body = r#"[{"ID":"a","VissibleName":"  ","Type":"DocumentType"}]"#;
        assert_eq!(parse_listing(body).expect("a listing")[0].name, "Untitled");
    }

    #[test]
    fn one_unreadable_entry_does_not_hide_the_rest() {
        let body = r#"[{"VissibleName":"no id here","Type":"DocumentType"},
                       {"ID":"b","VissibleName":"Notes","Type":"DocumentType"}]"#;
        let items = parse_listing(body).expect("a listing");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "b");
    }

    #[test]
    fn a_deleted_notebook_is_marked_rather_than_offered() {
        let body = r#"[{"ID":"a","VissibleName":"Old","Type":"DocumentType","Parent":"trash"}]"#;
        assert!(parse_listing(body).expect("a listing")[0].is_deleted());
    }

    #[test]
    fn an_empty_library_is_a_listing_not_an_error() {
        assert_eq!(parse_listing("[]").expect("a listing"), vec![]);
    }

    #[test]
    fn something_that_is_not_a_listing_says_so() {
        assert!(parse_listing("not json").is_err());
        assert!(parse_listing(r#"{"error":"nope"}"#).is_err());
    }

    #[test]
    fn a_notebook_title_cannot_write_outside_the_folder_it_is_saved_in() {
        assert_eq!(safe_filename("Q1 / planning"), "Q1 - planning");
        assert_eq!(safe_filename("../../etc/passwd"), "-..-etc-passwd");
        assert_eq!(safe_filename(".."), "Untitled");
        assert_eq!(safe_filename("   "), "Untitled");
        assert_eq!(safe_filename("Rust Book Notes"), "Rust Book Notes");
    }
}
