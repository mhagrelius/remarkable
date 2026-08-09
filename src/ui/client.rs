//! Talking to llama-server, and to the tablet, on the main loop.
//!
//! libsoup is the platform's HTTP client and its async calls complete on the
//! GLib main loop, so a section of a page is one `send_and_read_async` callback on
//! the thread that owns the widgets — no runtime, no channel, no worker thread
//! and no hand-off to get the text back into the view. Cancellation is a
//! `gio::Cancellable`, which is the same object the Stop button triggers.
//!
//! One session, not two. Unlike familiar there is nothing here that races a
//! long request: a document is read one section at a time, and the tablet is only
//! ever contacted between documents.

use gio::prelude::*;
use gtk::glib;
use soup::prelude::*;

use crate::model::wire::{self, ChatRequest, Completion, ServerInfo};

/// How long to let one request run.
///
/// A section of dense handwriting on a 27B model is twenty to forty seconds, and
/// the first request after the model is paged in is slower still. libsoup's
/// default of sixty would fail those, and a transcription that dies four sections
/// in is worse than one that takes its time.
const TIMEOUT_SECONDS: u32 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// Nothing answered. The server is asleep, the tablet is unplugged, or the
    /// address is wrong.
    Unreachable(String),
    /// It answered, with a refusal.
    Http { status: u16, body: String },
    /// It answered with something that could not be read.
    Malformed(String),
    /// It answered and then the connection broke.
    Transport(String),
    /// The Stop button.
    Cancelled,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "{detail}"),
            Self::Http { status, body } => {
                let body = body.trim();
                if body.is_empty() {
                    write!(f, "the server answered {status}")
                } else {
                    write!(f, "{body}")
                }
            }
            Self::Malformed(detail) | Self::Transport(detail) => write!(f, "{detail}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

pub struct Client {
    session: soup::Session,
    base_url: std::cell::RefCell<String>,
}

impl Client {
    pub fn new(base_url: &str) -> Self {
        Self {
            session: soup::Session::builder().timeout(TIMEOUT_SECONDS).build(),
            base_url: std::cell::RefCell::new(trimmed(base_url)),
        }
    }

    pub fn set_base_url(&self, base_url: &str) {
        self.base_url.replace(trimmed(base_url));
    }

    pub fn base_url(&self) -> String {
        self.base_url.borrow().clone()
    }

    /// Ask the server what it is: which model, and whether it can see.
    pub fn probe<F>(&self, on_answer: F)
    where
        F: FnOnce(Result<ServerInfo, ClientError>) + 'static,
    {
        let url = format!("{}/props", self.base_url.borrow());
        get(&self.session, &url, None, move |result| {
            on_answer(result.map(|body| wire::parse_props(&String::from_utf8_lossy(&body))));
        });
    }

    /// Transcribe one section. The returned `Cancellable` abandons it.
    pub fn transcribe<F>(&self, request: &ChatRequest, on_answer: F) -> gio::Cancellable
    where
        F: FnOnce(Result<Completion, ClientError>) + 'static,
    {
        let cancellable = gio::Cancellable::new();

        let body = match serde_json::to_vec(request) {
            Ok(body) => body,
            Err(error) => {
                on_answer(Err(ClientError::Malformed(error.to_string())));
                return cancellable;
            }
        };

        let url = format!("{}/v1/chat/completions", self.base_url.borrow());
        let message = match soup::Message::new("POST", &url) {
            Ok(message) => message,
            Err(_) => {
                on_answer(Err(ClientError::Unreachable(format!(
                    "{url} is not an address"
                ))));
                return cancellable;
            }
        };
        message.set_request_body_from_bytes(
            Some("application/json"),
            Some(&glib::Bytes::from_owned(body)),
        );

        send(&self.session, &message, Some(&cancellable), move |result| {
            on_answer(result.and_then(|body| {
                wire::parse_completion(&String::from_utf8_lossy(&body))
                    .map_err(ClientError::Malformed)
            }));
        });

        cancellable
    }

    /// A plain GET, for the tablet's listings and PDFs. The body is handed over
    /// as bytes — a PDF is not text.
    pub fn fetch<F>(&self, url: &str, on_answer: F) -> gio::Cancellable
    where
        F: FnOnce(Result<Vec<u8>, ClientError>) + 'static,
    {
        let cancellable = gio::Cancellable::new();
        get(&self.session, url, Some(&cancellable), on_answer);
        cancellable
    }
}

fn trimmed(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn get<F>(session: &soup::Session, url: &str, cancellable: Option<&gio::Cancellable>, on_answer: F)
where
    F: FnOnce(Result<Vec<u8>, ClientError>) + 'static,
{
    let Ok(message) = soup::Message::new("GET", url) else {
        on_answer(Err(ClientError::Unreachable(format!(
            "{url} is not an address"
        ))));
        return;
    };
    send(session, &message, cancellable, on_answer);
}

/// Send a message and hand the whole body to `on_answer`.
///
/// A non-2xx carries its body, because the interesting part of llama-server's
/// refusal is the JSON explaining it — "this model has no vision projector"
/// rather than "400".
fn send<F>(
    session: &soup::Session,
    message: &soup::Message,
    cancellable: Option<&gio::Cancellable>,
    on_answer: F,
) where
    F: FnOnce(Result<Vec<u8>, ClientError>) + 'static,
{
    // Cloned for the callback: the status has to be read after the send
    // completes, through a handle the closure owns.
    let sent = message.clone();
    let owned = cancellable.cloned();
    session.send_and_read_async(
        message,
        glib::Priority::DEFAULT,
        cancellable,
        move |result| {
            let outcome = match result {
                Ok(bytes) => {
                    let status = sent.status_code() as u16;
                    if (200..300).contains(&status) {
                        Ok(bytes.to_vec())
                    } else {
                        Err(ClientError::Http {
                            status,
                            body: String::from_utf8_lossy(&bytes).to_string(),
                        })
                    }
                }
                Err(error) => Err(classify(&error, owned.as_ref())),
            };
            on_answer(outcome);
        },
    );
}

fn classify(error: &glib::Error, cancellable: Option<&gio::Cancellable>) -> ClientError {
    if cancellable.is_some_and(gio::Cancellable::is_cancelled)
        || error.matches(gio::IOErrorEnum::Cancelled)
    {
        return ClientError::Cancelled;
    }
    if error.matches(gio::IOErrorEnum::ConnectionRefused)
        || error.matches(gio::IOErrorEnum::HostNotFound)
        || error.matches(gio::IOErrorEnum::HostUnreachable)
        || error.matches(gio::IOErrorEnum::NetworkUnreachable)
        || error.matches(gio::IOErrorEnum::TimedOut)
    {
        return ClientError::Unreachable(error.message().to_string());
    }
    ClientError::Transport(error.message().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_url_is_normalised_once_so_paths_do_not_double_their_slashes() {
        let client = Client::new("  http://127.0.0.1:8080/  ");
        assert_eq!(client.base_url(), "http://127.0.0.1:8080");
        client.set_base_url("http://localhost:9090///");
        assert_eq!(client.base_url(), "http://localhost:9090");
    }

    #[test]
    fn an_error_reads_as_something_a_person_can_act_on() {
        assert_eq!(
            ClientError::Unreachable("connection refused".into()).to_string(),
            "connection refused"
        );
        assert_eq!(
            ClientError::Http {
                status: 400,
                body: "  ".into()
            }
            .to_string(),
            "the server answered 400"
        );
        assert_eq!(
            ClientError::Http {
                status: 400,
                body: r#"{"error":"no vision projector"}"#.into()
            }
            .to_string(),
            r#"{"error":"no vision projector"}"#
        );
    }
}
