//! What the model is asked.
//!
//! One base instruction, in `prompts/transcribe.md`, plus a note about where
//! this section sits on the page. The note is the whole reason a tall notebook
//! comes out readable: a section cut out of the middle of a page has writing
//! running off both edges, and a model not told so will invent the ends of
//! those lines rather than leave them to the neighbouring section.
//!
//! The tail of the previous section's transcript rides along too. It is context,
//! not content — it tells the model what it has already covered so it can start
//! where that left off, which makes [`super::merge`]'s job a tidy seam rather
//! than a guess.

use super::sections::Section;

/// The instruction, verbatim.
///
/// Public so the suite can prove no anchor it grades on appears in here — see
/// `eval::suite`. A phrase the prompt hands the model is a phrase the model can
/// produce without reading the page.
pub const BASE: &str = include_str!("prompts/transcribe.md");

/// How many lines of the previous section to show as context. Enough to place the
/// model on the page; not so much that it starts transcribing them again.
const CONTEXT_LINES: usize = 10;

/// The instruction for one section of a page.
///
/// `previous` is the transcript of the section above, if there was one.
pub fn for_section(section: &Section, previous: Option<&str>) -> String {
    if section.total <= 1 {
        return BASE.to_string();
    }

    let mut prompt = String::with_capacity(BASE.len() + 1024);
    prompt.push_str(BASE);
    prompt.push_str("\n### Where this image sits\n\n");

    let (position, edges) = match (section.overlaps_above(), section.overlaps_below()) {
        (false, _) => (
            format!(
                "This is the top of a page, section 1 of {}. The writing continues below it.",
                section.total
            ),
            "The **bottom edge** may cut a line in half. Transcribe only what is fully visible \
             there and stop — do not complete a partial word. The next section covers it.",
        ),
        (_, false) => (
            format!(
                "This is the bottom of a page, section {} of {}. The writing started above it.",
                section.index + 1,
                section.total
            ),
            "The **top edge** repeats the end of the previous section. Continue from where that \
             left off rather than transcribing those lines again.",
        ),
        _ => (
            format!(
                "This is the middle of a page, section {} of {}.",
                section.index + 1,
                section.total
            ),
            "**Both edges** are shared with the neighbouring sections: the top repeats the end of \
             the previous one, and the bottom may cut a line in half. Transcribe the clearly \
             visible content between them.",
        ),
    };

    prompt.push_str(&position);
    prompt.push('\n');
    prompt.push_str(edges);
    prompt.push('\n');

    if let Some(previous) = previous.filter(|_| section.overlaps_above()) {
        if let Some(tail) = tail_of(previous) {
            prompt.push_str(
                "\nThe previous section ended with the following. This is for your reference \
                 only — it may contain its own mistakes, and you should not repeat them or \
                 transcribe these lines again.\n\n",
            );
            prompt.push_str("```\n");
            prompt.push_str(&tail);
            prompt.push_str("\n```\n");
        }
    }

    prompt
}

/// The last few non-empty lines of a transcript.
fn tail_of(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let from = lines.len().saturating_sub(CONTEXT_LINES);
    Some(lines[from..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(index: usize, total: usize) -> Section {
        Section {
            top: 0,
            bottom: 100,
            index,
            total,
        }
    }

    #[test]
    fn a_page_that_was_not_cut_gets_the_instruction_and_nothing_else() {
        assert_eq!(for_section(&section(0, 1), None), BASE);
    }

    #[test]
    fn the_base_instruction_asks_for_what_the_pipeline_relies_on() {
        // `text::split_footer` looks for the marker, `text::is_blank` for the
        // blank page, `text::clean` strips the fence, and the eval scores the
        // diagram lines. Reword past any of these and the pipeline degrades
        // silently rather than failing.
        assert!(BASE.contains(crate::model::text::FOOTER_MARKER));
        assert!(BASE.contains("[blank page]"));
        assert!(BASE.contains("[Diagram:"));
        assert!(BASE.contains("code fence"));
        // The one marker the model still writes inline, for text it cannot
        // read at all — there is no word to put in the footer.
        assert!(BASE.contains("[unclear: ???]"));
    }

    #[test]
    fn the_first_section_is_warned_about_its_bottom_edge_only() {
        let prompt = for_section(&section(0, 4), None);
        assert!(prompt.contains("top of a page, section 1 of 4"));
        assert!(prompt.contains("**bottom edge**"));
        assert!(!prompt.contains("**top edge**"));
    }

    #[test]
    fn a_middle_section_is_warned_about_both() {
        let prompt = for_section(&section(1, 4), None);
        assert!(prompt.contains("middle of a page, section 2 of 4"));
        assert!(prompt.contains("**Both edges**"));
    }

    #[test]
    fn the_last_section_is_told_the_writing_started_above() {
        let prompt = for_section(&section(3, 4), None);
        assert!(prompt.contains("bottom of a page, section 4 of 4"));
        assert!(prompt.contains("**top edge**"));
        assert!(!prompt.contains("cut a line in half"));
    }

    #[test]
    fn the_previous_sections_tail_is_carried_as_reference() {
        let prompt = for_section(&section(1, 3), Some("- one\n- two\n- three"));
        assert!(prompt.contains("previous section ended"));
        assert!(prompt.contains("- three"));
    }

    #[test]
    fn only_the_tail_is_carried_not_the_whole_section() {
        let previous: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let prompt = for_section(&section(1, 3), Some(&previous));
        assert!(prompt.contains("line 39"));
        assert!(!prompt.contains("line 20"));
    }

    #[test]
    fn the_first_section_is_not_given_a_previous_section() {
        let prompt = for_section(&section(0, 3), Some("- one\n- two"));
        assert!(!prompt.contains("previous section ended"));
    }

    #[test]
    fn a_previous_section_that_read_nothing_adds_no_reference_block() {
        let prompt = for_section(&section(1, 3), Some("   \n\n  "));
        assert!(!prompt.contains("previous section ended"));
    }
}
