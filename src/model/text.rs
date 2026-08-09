//! Tidying what the model returns.
//!
//! Deliberately conservative. The Python this was ported from also ran a table
//! of "OCR corrections" — `rn` to `m`, `0` to `O` before a letter, `0` to `o`
//! after one — which were written for a classical OCR engine that confuses
//! glyph shapes. A vision-language model does not make those mistakes, and on
//! these notebooks the rules only fired on text that was already right: `P0`
//! became `Po`, `0x1F` became `Ox1F`. Guessing at the model's output after the
//! fact cannot beat asking the model to mark its own uncertainty, which is what
//! the prompt does. So they are gone, and what is left only removes things the
//! model added.
//!
//! The one rule with teeth concerns leading whitespace, which is list nesting.
//! The Python collapsed it with a blanket two-or-more-spaces rule, flattening
//! every sub-bullet on the page. Here only the indent *common to every line* is
//! removed, which drops a uniform shift the model added — four spaces of it
//! would otherwise render as a code block — while leaving the relative nesting
//! underneath exactly as it was.

/// What separates a section's transcription from its list of doubts.
///
/// The prompt asks for the doubts as a trailing section rather than as inline
/// `[unclear: ...]` markers, and that is not a stylistic choice. Asked to hedge
/// inline, Qwen3.6 with thinking disabled marks nothing at all, across every
/// wording tried — its transcription prior is to emit clean text and it will
/// not interrupt itself. Asked for a list at the end, it produces one,
/// reproducibly, and the list catches real misreadings. Format compliance is a
/// different capability from mid-flow self-doubt, and only one of them is
/// available here.
pub const FOOTER_MARKER: &str = "---UNCERTAIN---";

/// What the model writes when it doubted nothing.
const NOTHING_DOUBTED: &str = "none";

/// Markers a model uses when there is nothing on the page.
const BLANK_MARKERS: [&str; 5] = [
    "[blank page]",
    "[blank]",
    "[empty]",
    "[no text]",
    "[no text detected]",
];

/// Remove the wrapper a model puts around an answer, and normalise the
/// whitespace between blocks.
pub fn clean(raw: &str) -> String {
    // Only the fence check works on the trimmed text. Trimming the answer
    // itself would eat the leading spaces of its first line, and a section that
    // opens on a sub-bullet would lose a level of nesting at every seam.
    let trimmed = raw.trim();
    let text = if trimmed.starts_with("```") {
        strip_fence(trimmed)
    } else {
        raw
    };

    // Indentation is nesting, but only *relative* nesting. A section whose every
    // line is shifted right by two — which happens when the model mirrors where
    // the writing sits on the page — should not lose its sub-bullets, and
    // should not become an indented code block either, which is what four
    // uniform leading spaces mean in Markdown. Removing the common indent
    // settles both.
    let common = common_indent(text);

    let mut out: Vec<String> = Vec::new();
    let mut blank_run = 0usize;

    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            // One blank line separates blocks; more is the model padding.
            if blank_run == 1 && !out.is_empty() {
                out.push(String::new());
            }
            continue;
        }
        blank_run = 0;
        out.push(collapse_inner_spaces(&line[common.min(indent_of(line))..]));
    }

    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// The indentation every non-empty line shares.
fn common_indent(text: &str) -> usize {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(indent_of)
        .min()
        .unwrap_or(0)
}

/// How many leading spaces a line has. Tabs count as one — a model asked for
/// two-space nesting does not emit them, and treating one as eight would
/// mangle the rare line that has one.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Take the text out of a ```` ``` ```` fence, if the model wrapped it in one
/// despite being told not to.
///
/// Only an opening fence on the very first line counts. A fence in the middle
/// is the author's own code block — the samples include `fn main() {}` copied
/// out of the Rust book — and removing that would lose the page's meaning.
fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // The info string on the opening fence: ```markdown, ```md, or nothing.
    let Some((_, body)) = rest.split_once('\n') else {
        return text;
    };
    match body.trim_end().strip_suffix("```") {
        Some(inner) => inner.trim_end(),
        // An opening fence with no closing one. The model started a block and
        // ran out of tokens; the text inside is still the answer.
        None => body,
    }
}

/// Squeeze runs of spaces *within* a line, leaving its indentation alone.
fn collapse_inner_spaces(line: &str) -> String {
    let indent_to = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_to);

    let mut out = String::with_capacity(line.len());
    out.push_str(indent);
    let mut previous_was_space = false;
    for c in rest.chars() {
        let is_space = c == ' ';
        if !(is_space && previous_was_space) {
            out.push(c);
        }
        previous_was_space = is_space;
    }
    out
}

/// Split a section's answer into its transcription and the words it doubted.
///
/// A missing footer is not an error — it means the model dropped the
/// instruction on this section, which happens — and yields an empty list rather
/// than swallowing the transcription.
pub fn split_footer(raw: &str) -> (&str, Vec<&str>) {
    let Some((body, footer)) = raw.split_once(FOOTER_MARKER) else {
        return (raw, Vec::new());
    };

    let doubts = footer
        .lines()
        .map(str::trim)
        // The model sometimes bullets the list despite being asked not to.
        .map(|line| line.trim_start_matches(['-', '*', '•']).trim())
        .filter(|line| !line.is_empty())
        .filter(|line| !line.eq_ignore_ascii_case(NOTHING_DOUBTED))
        // A "word" longer than this is the model explaining itself rather than
        // naming a token, and marking a whole sentence helps nobody.
        .filter(|line| line.len() <= 48)
        .collect();

    (body, doubts)
}

/// Mark the words the model listed, where they appear in the text.
///
/// The marker the rest of the app reads is `[unclear: word]`, so the footer is
/// turned into those rather than kept as a list: inline is where a reader needs
/// the warning, and every other part of this crate — the eval's count, the
/// window, the saved Markdown — already speaks that language.
///
/// Only whole words are marked, and only outside an existing marker, so running
/// this twice changes nothing.
pub fn mark_doubts(text: &str, doubts: &[&str]) -> String {
    let mut marked = text.to_string();

    for doubt in doubts {
        let doubt = doubt.trim();
        // A single character matches half the page. Two is fine — `IT` and
        // `AI` are real acronyms, and whole-word matching already stops them
        // firing inside `monitoring`. `???` is a marker in its own right.
        if doubt.chars().count() < 2 || doubt.contains("unclear") {
            continue;
        }
        marked = mark_one(&marked, doubt);
    }

    marked
}

fn mark_one(text: &str, doubt: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;

    while let Some(at) = rest.find(doubt) {
        let (before, from) = rest.split_at(at);
        let (hit, after) = from.split_at(doubt.len());

        let whole_word = !before.chars().next_back().is_some_and(is_word_char)
            && !after.chars().next().is_some_and(is_word_char);
        // Already inside `[unclear: ...]`, from a previous term or from the
        // model's own illegible marker.
        let already_marked = before.rfind('[').is_some_and(|open| {
            before[open..].starts_with("[unclear:") && !before[open..].contains(']')
        });

        out.push_str(before);
        if whole_word && !already_marked {
            out.push_str("[unclear: ");
            out.push_str(hit);
            out.push(']');
        } else {
            out.push_str(hit);
        }
        rest = after;
    }

    out.push_str(rest);
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether a transcript says the page had nothing on it.
pub fn is_blank(text: &str) -> bool {
    let text = text.trim().to_lowercase();
    text.is_empty() || BLANK_MARKERS.contains(&text.as_str())
}

/// How many places the model said it was unsure.
///
/// The prompt asks for `[unclear: guess]` and says that a page of handwriting
/// with none of them means it guessed silently. Counting them is how the eval
/// checks that instruction survived.
pub fn unclear_markers(text: &str) -> usize {
    text.match_indices("[unclear:").count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_markdown_fence_around_the_whole_answer_is_removed() {
        assert_eq!(clean("```markdown\n# Page\n\ntext\n```"), "# Page\n\ntext");
        assert_eq!(clean("```\n# Page\n```"), "# Page");
    }

    #[test]
    fn an_unclosed_fence_does_not_eat_the_answer() {
        assert_eq!(
            clean("```markdown\n# Page\n\nsome text"),
            "# Page\n\nsome text"
        );
    }

    #[test]
    fn a_code_block_the_author_wrote_is_left_alone() {
        // From the Rust book page in the samples.
        let page = "Ch1 Define a Function\n\n```rust\nfn main() {\n}\n```\n\n- use rustfmt";
        assert_eq!(clean(page), page);
    }

    #[test]
    fn list_indentation_survives() {
        let nested = "- Next 6 months\n  - CMDB/Change management\n    - Headless runs IT";
        assert_eq!(clean(nested), nested);
    }

    #[test]
    fn runs_of_spaces_inside_a_line_are_squeezed() {
        assert_eq!(
            clean("- stay    away  from   servers"),
            "- stay away from servers"
        );
    }

    #[test]
    fn an_indent_shared_by_every_line_is_removed_but_the_nesting_under_it_is_not() {
        // Four uniform leading spaces would render as a code block; the two
        // extra on the second line are a real sub-bullet.
        assert_eq!(
            clean("    - Notifications\n      - Teams mobility"),
            "- Notifications\n  - Teams mobility"
        );
    }

    #[test]
    fn a_section_that_opens_on_a_sub_bullet_keeps_it_under_its_parent() {
        let nested = "- Next 6 months\n  - CMDB\n    - Headless runs IT";
        assert_eq!(clean(nested), nested);
    }

    #[test]
    fn padding_between_blocks_is_reduced_to_one_blank_line() {
        assert_eq!(clean("a\n\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn surrounding_whitespace_goes() {
        assert_eq!(clean("\n\n  text  \n\n\n"), "text");
        assert_eq!(clean("   "), "");
    }

    #[test]
    fn trailing_whitespace_goes_but_a_hard_break_is_not_invented() {
        assert_eq!(clean("line one   \nline two"), "line one\nline two");
    }

    #[test]
    fn a_blank_page_is_recognised_however_it_is_marked() {
        assert!(is_blank(""));
        assert!(is_blank("  \n "));
        assert!(is_blank("[blank page]"));
        assert!(is_blank("[Blank Page]"));
        assert!(is_blank("[no text detected]"));
        assert!(!is_blank("- Interns?"));
    }

    #[test]
    fn a_page_number_is_not_a_blank_page() {
        assert!(!is_blank("[Sketch of a flow diagram]"));
    }

    #[test]
    fn the_footer_is_split_off_and_never_reaches_the_transcript() {
        let answer = "- a bullet\n- another\n\n---UNCERTAIN---\nCMOB\nGraybar\n";
        let (body, doubts) = split_footer(answer);
        assert!(!body.contains("UNCERTAIN"));
        assert_eq!(body.trim(), "- a bullet\n- another");
        assert_eq!(doubts, vec!["CMOB", "Graybar"]);
    }

    #[test]
    fn a_section_with_no_footer_keeps_all_of_its_text() {
        let (body, doubts) = split_footer("- a bullet\n- another");
        assert_eq!(body, "- a bullet\n- another");
        assert!(doubts.is_empty());
    }

    #[test]
    fn a_footer_saying_none_lists_nothing() {
        let (_, doubts) = split_footer("text\n---UNCERTAIN---\nnone\n");
        assert!(doubts.is_empty());
        let (_, doubts) = split_footer("text\n---UNCERTAIN---\nNone");
        assert!(doubts.is_empty());
    }

    #[test]
    fn a_bulleted_footer_is_read_anyway() {
        let (_, doubts) = split_footer("text\n---UNCERTAIN---\n- CMOB\n* Graybar\n");
        assert_eq!(doubts, vec!["CMOB", "Graybar"]);
    }

    #[test]
    fn the_model_explaining_itself_is_not_treated_as_a_word() {
        let long = "I was not certain about several of the acronyms on this page at all";
        let answer = format!("text\n---UNCERTAIN---\nCMOB\n{long}\n");
        let (_, doubts) = split_footer(&answer);
        assert_eq!(doubts, vec!["CMOB"]);
    }

    #[test]
    fn listed_words_become_markers_where_they_appear() {
        let text = "CMOB/Change management\n- Graybar GIA missing SAP";
        let marked = mark_doubts(text, &["CMOB", "Graybar"]);
        assert!(marked.contains("[unclear: CMOB]/Change"));
        assert!(marked.contains("- [unclear: Graybar] GIA"));
        assert_eq!(unclear_markers(&marked), 2);
    }

    #[test]
    fn only_whole_words_are_marked() {
        // "IT" must not fire inside "monitoring" or "IT's".
        let text = "Headless runs IT\nmonitoring the site";
        let marked = mark_doubts(text, &["IT"]);
        assert!(marked.contains("runs [unclear: IT]"), "{marked}");
        assert!(marked.contains("monitoring the site"), "{marked}");
    }

    #[test]
    fn a_word_the_model_did_not_actually_write_marks_nothing() {
        let text = "- a plain bullet";
        assert_eq!(mark_doubts(text, &["Ravenscroft"]), text);
    }

    #[test]
    fn marking_is_idempotent_and_does_not_nest() {
        let text = "CMOB/Change management";
        let once = mark_doubts(text, &["CMOB"]);
        let twice = mark_doubts(&once, &["CMOB"]);
        assert_eq!(once, twice);
        assert_eq!(unclear_markers(&twice), 1);
    }

    #[test]
    fn the_models_own_illegible_marker_is_left_alone() {
        let text = "the [unclear: ???] and the rest";
        assert_eq!(mark_doubts(text, &["???"]), text);
    }

    #[test]
    fn a_term_too_short_to_be_meaningful_is_ignored() {
        let text = "a b c and a longer line";
        assert_eq!(mark_doubts(text, &["a", "of"]), text);
    }

    #[test]
    fn every_occurrence_of_a_doubted_word_is_marked() {
        let text = "GIA missing SAP\nchase GIA tomorrow";
        assert_eq!(unclear_markers(&mark_doubts(text, &["GIA"])), 2);
    }

    #[test]
    fn uncertainty_markers_are_counted() {
        assert_eq!(unclear_markers("plain text"), 0);
        assert_eq!(
            unclear_markers("[unclear: Sneha], [unclear: Anisha] under [unclear: maximo]"),
            3
        );
    }

    #[test]
    fn the_old_glyph_corrections_no_longer_corrupt_correct_text() {
        // Every one of these was mangled by the Python's correction table.
        assert_eq!(
            clean("P0 incident, 0x1F, morning"),
            "P0 incident, 0x1F, morning"
        );
    }
}
