//! The finished document, in the three shapes it can be saved as.
//!
//! Markdown is the one that matters — it is what a transcript is *for*, and it
//! is written to drop straight into a brain vault, which is why the frontmatter
//! is the same YAML-in-`---` brain reads. JSON is for anything downstream that
//! wants the pages apart. Plain text is for pasting somewhere that will not
//! render either.

use std::fmt::Write as _;

use serde::Serialize;

/// One page of a source document, transcribed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageText {
    pub number: usize,
    pub text: String,
    /// How many sections the page was cut into. One means it was read whole.
    pub sections: usize,
}

/// A whole document, ready to write out.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transcript {
    /// The file it came from, by name only — the path is the user's business
    /// and does not belong in a note they may share.
    pub source: String,
    pub model: String,
    /// RFC 3339, in UTC.
    pub processed: String,
    pub pages: Vec<PageText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
    Text,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
            Self::Text => "txt",
        }
    }
}

impl Transcript {
    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Markdown => self.markdown(),
            Format::Json => self.json(),
            Format::Text => self.text(),
        }
    }

    /// The transcript, with nothing around it. What the window shows and the
    /// Copy button copies — a heading and a frontmatter block are for a file,
    /// not for a paste into a chat window.
    pub fn body(&self) -> String {
        self.pages
            .iter()
            .map(|page| page.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn markdown(&self) -> String {
        let mut out = String::with_capacity(self.pages.iter().map(|p| p.text.len() + 32).sum());

        out.push_str("---\n");
        // Quoted: a reMarkable notebook is named by its author and "Q1: plans"
        // is not valid YAML unquoted.
        let _ = writeln!(out, "source: {}", quote(&self.source));
        let _ = writeln!(out, "processed: {}", self.processed);
        let _ = writeln!(out, "pages: {}", self.pages.len());
        let _ = writeln!(out, "model: {}", quote(&self.model));
        out.push_str("---\n\n");

        // A single-page export — which is what a reMarkable notebook is — gets
        // no page heading. Numbering the only page is noise, and the transcript
        // reads as one note.
        let numbered = self.pages.len() > 1;
        for (position, page) in self.pages.iter().enumerate() {
            if position > 0 {
                out.push_str("\n\n");
            }
            if numbered {
                let _ = writeln!(out, "## Page {}\n", page.number);
            }
            out.push_str(page.text.trim());
            out.push('\n');
        }

        out
    }

    fn json(&self) -> String {
        // Pretty: a transcript is read by people at least as often as by
        // programs, and a wall of escaped newlines is read by neither.
        serde_json::to_string_pretty(self).unwrap_or_else(|error| {
            // Serializing a struct of owned strings cannot fail for any reason
            // the caller can act on, but a panic here would lose the
            // transcription that took two minutes to produce.
            format!("{{\"error\":{}}}", quote(&error.to_string()))
        })
    }

    fn text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{} — transcribed {}\n", self.source, self.processed);

        let numbered = self.pages.len() > 1;
        for page in &self.pages {
            if numbered {
                let _ = writeln!(out, "--- Page {} ---\n", page.number);
            }
            out.push_str(page.text.trim());
            out.push_str("\n\n");
        }
        out.truncate(out.trim_end().len());
        out.push('\n');
        out
    }

    /// The name to save under, given the source file's stem.
    pub fn filename(stem: &str, format: Format) -> String {
        let stem = stem.trim();
        let stem = if stem.is_empty() { "transcript" } else { stem };
        format!("{stem}.{}", format.extension())
    }
}

/// A JSON string, which is also a valid double-quoted YAML scalar.
fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(pages: &[&str]) -> Transcript {
        Transcript {
            source: "notes.png".into(),
            model: "qwen3.6-27b".into(),
            processed: "2026-08-02T18:30:00Z".into(),
            pages: pages
                .iter()
                .enumerate()
                .map(|(i, text)| PageText {
                    number: i + 1,
                    text: (*text).to_string(),
                    sections: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn markdown_opens_with_frontmatter_a_vault_can_read() {
        let out = transcript(&["- a bullet"]).render(Format::Markdown);
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("---"));
        assert_eq!(lines.next(), Some(r#"source: "notes.png""#));
        assert_eq!(lines.next(), Some("processed: 2026-08-02T18:30:00Z"));
        assert_eq!(lines.next(), Some("pages: 1"));
        assert_eq!(lines.next(), Some(r#"model: "qwen3.6-27b""#));
        assert_eq!(lines.next(), Some("---"));
    }

    #[test]
    fn a_name_that_would_break_yaml_is_quoted() {
        let mut source = transcript(&["text"]);
        source.source = "Q1: plans #2.png".into();
        assert!(source
            .render(Format::Markdown)
            .contains(r#"source: "Q1: plans #2.png""#));
    }

    #[test]
    fn a_single_page_export_is_not_given_a_page_heading() {
        let out = transcript(&["- a bullet"]).render(Format::Markdown);
        assert!(!out.contains("## Page"));
        assert!(out.ends_with("- a bullet\n"));
    }

    #[test]
    fn a_multi_page_document_numbers_its_pages() {
        let out = transcript(&["first", "second"]).render(Format::Markdown);
        assert!(out.contains("## Page 1\n"));
        assert!(out.contains("## Page 2\n"));
        assert!(out.contains("first"));
        assert!(out.contains("second"));
    }

    #[test]
    fn the_body_is_the_transcript_with_nothing_around_it() {
        let out = transcript(&["first", "second"]).body();
        assert_eq!(out, "first\n\nsecond");
        assert!(!out.contains("---"));
        assert!(!out.contains("## Page"));
    }

    #[test]
    fn a_blank_page_does_not_leave_a_hole_in_the_body() {
        assert_eq!(
            transcript(&["first", "  ", "third"]).body(),
            "first\n\nthird"
        );
    }

    #[test]
    fn json_keeps_the_pages_apart() {
        let out = transcript(&["first", "second"]).render(Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed["source"], "notes.png");
        assert_eq!(parsed["model"], "qwen3.6-27b");
        assert_eq!(parsed["pages"][0]["number"], 1);
        assert_eq!(parsed["pages"][1]["text"], "second");
    }

    #[test]
    fn plain_text_marks_its_pages_and_ends_with_one_newline() {
        let out = transcript(&["first", "second"]).render(Format::Text);
        assert!(out.starts_with("notes.png — transcribed 2026-08-02T18:30:00Z"));
        assert!(out.contains("--- Page 2 ---"));
        assert!(out.ends_with("second\n"));
        assert!(!out.ends_with("\n\n"));
    }

    #[test]
    fn a_saved_file_is_named_after_the_source() {
        assert_eq!(Transcript::filename("notes", Format::Markdown), "notes.md");
        assert_eq!(Transcript::filename("notes", Format::Json), "notes.json");
        assert_eq!(Transcript::filename("notes", Format::Text), "notes.txt");
    }

    #[test]
    fn a_source_with_no_name_still_saves_somewhere() {
        assert_eq!(
            Transcript::filename("  ", Format::Markdown),
            "transcript.md"
        );
    }
}
