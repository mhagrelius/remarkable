//! Did it read what is on the page?
//!
//! Full ground truth for a seventeen-thousand-pixel notebook is a day of typing
//! and goes stale the moment the prompt changes. What is used instead is a set
//! of **anchors**: phrases read off the image by a person, spread through the
//! document, that a correct transcription must contain. Recall against those is
//! a real accuracy number — it catches a section that was skipped, a seam that ate
//! a paragraph, and a model that summarised instead of transcribing, which are
//! the three failures that matter.
//!
//! What it does not catch is a word invented in a place no anchor covers. That
//! is what the `[unclear:]` count in [`super::format`] is for.
//!
//! Matching is loose on purpose. `- **WealthSpire**` and `WealthSpire` are the
//! same reading, and an anchor that failed on the bolding would be measuring
//! the formatter rather than the transcription.

/// A phrase that must appear, and whether it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub anchor: &'static str,
    pub found: bool,
}

/// How much of what is on the page came back.
#[derive(Debug, Clone, PartialEq)]
pub struct Recall {
    pub hits: Vec<Hit>,
}

impl Recall {
    pub fn found(&self) -> usize {
        self.hits.iter().filter(|hit| hit.found).count()
    }

    pub fn total(&self) -> usize {
        self.hits.len()
    }

    pub fn fraction(&self) -> f64 {
        if self.hits.is_empty() {
            return 1.0;
        }
        self.found() as f64 / self.total() as f64
    }

    pub fn missing(&self) -> Vec<&'static str> {
        self.hits
            .iter()
            .filter(|hit| !hit.found)
            .map(|hit| hit.anchor)
            .collect()
    }
}

/// Check a transcript against the anchors for its page.
pub fn recall(transcript: &str, anchors: &[&'static str]) -> Recall {
    let spaced = flatten(transcript);
    // Where a space falls inside a compound is orthography, not a reading:
    // `AlignTech` and `Align Tech` are the same word off the page, as are
    // `burndown` and `burn down`. Scoring those as misses measures how the
    // anchor was typed rather than how the page was read, so the space-free
    // form is tried as well.
    let solid: String = spaced.chars().filter(|c| *c != ' ').collect();

    Recall {
        hits: anchors
            .iter()
            .map(|anchor| {
                let needle = flatten(anchor);
                let solid_needle: String = needle.chars().filter(|c| *c != ' ').collect();
                Hit {
                    anchor,
                    found: spaced.contains(&needle) || solid.contains(&solid_needle),
                }
            })
            .collect(),
    }
}

/// Reduce text to the letters, digits and single spaces in it.
///
/// Markdown decoration, punctuation and line wrapping all disappear, so an
/// anchor matches whether the model bolded the phrase, bulleted it, or wrapped
/// it across two lines.
fn flatten(text: &str) -> String {
    // Our own uncertainty marker is not page content, and it sits *inside*
    // phrases: `use [unclear: rustfmt] for formatting` would otherwise flatten
    // with "unclear" wedged between "use" and "rustfmt" and stop matching the
    // anchor. A word the model read correctly and then flagged has still been
    // read correctly, which is what this is measuring — flagging is scored
    // separately, by the marker count.
    let text = text.replace("[unclear:", " ");

    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_that_is_there_is_found() {
        let recall = recall("- WealthSpire\n- TopBuild", &["WealthSpire", "TopBuild"]);
        assert_eq!(recall.found(), 2);
        assert_eq!(recall.fraction(), 1.0);
        assert!(recall.missing().is_empty());
    }

    #[test]
    fn an_anchor_that_is_not_is_reported_by_name() {
        let recall = recall("- WealthSpire", &["WealthSpire", "Huntington"]);
        assert_eq!(recall.found(), 1);
        assert_eq!(recall.fraction(), 0.5);
        assert_eq!(recall.missing(), vec!["Huntington"]);
    }

    #[test]
    fn markdown_decoration_does_not_hide_an_anchor() {
        assert_eq!(
            recall("- **WealthSpire** is next", &["WealthSpire"]).found(),
            1
        );
        assert_eq!(recall("## Current Needs", &["Current Needs"]).found(), 1);
    }

    #[test]
    fn punctuation_and_case_do_not_hide_an_anchor() {
        assert_eq!(
            recall("Cargo.toml holds config", &["cargo toml"]).found(),
            1
        );
        assert_eq!(recall("println!() is a macro", &["println"]).found(), 1);
        assert_eq!(recall("- F & B", &["F & B"]).found(), 1);
    }

    #[test]
    fn a_phrase_wrapped_across_two_lines_is_still_found() {
        let wrapped = "- All workspaces have an email\n  folder";
        assert_eq!(
            recall(wrapped, &["All workspaces have an email folder"]).found(),
            1
        );
    }

    #[test]
    fn a_word_that_was_read_correctly_and_then_flagged_still_counts() {
        // The marker sits inside the phrase, so a naive flatten would wedge
        // "unclear" between the two words and lose the anchor.
        assert_eq!(
            recall(
                "use [unclear: rustfmt] for formatting",
                &["use rustfmt for formatting"]
            )
            .found(),
            1
        );
        assert_eq!(
            recall("- [unclear: WealthSpire]", &["WealthSpire"]).found(),
            1
        );
    }

    #[test]
    fn a_word_the_model_marked_as_illegible_is_still_a_miss() {
        assert_eq!(
            recall("the [unclear: ???] section", &["WealthSpire"]).found(),
            0
        );
    }

    #[test]
    fn a_space_inside_a_compound_does_not_decide_a_match() {
        assert_eq!(recall("## Align Tech", &["AlignTech"]).found(), 1);
        assert_eq!(recall("## AlignTech", &["Align Tech"]).found(), 1);
        assert_eq!(
            recall("**DEERE** create burn down list", &["create burndown list"]).found(),
            1
        );
    }

    #[test]
    fn an_anchor_is_not_matched_by_its_letters_scattered_about() {
        // "cargo run" must be those two words together, not "cargo" somewhere
        // and "run" somewhere else.
        let transcript = "- cargo new creates a project\n- Headless runs IT";
        assert_eq!(recall(transcript, &["cargo run"]).found(), 0);
    }

    #[test]
    fn an_empty_transcript_finds_nothing() {
        assert_eq!(recall("", &["anything"]).found(), 0);
        assert_eq!(recall("", &["anything"]).fraction(), 0.0);
    }

    #[test]
    fn a_page_with_no_anchors_is_not_a_division_by_zero() {
        assert_eq!(recall("some text", &[]).fraction(), 1.0);
    }
}
