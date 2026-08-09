//! Grading a transcription.
//!
//! Two halves, deliberately. [`format`] asks whether the output is well-formed
//! Markdown and needs nothing but the output. [`expect`] asks whether it says
//! what is on the page, and needs anchors — phrases a person read off the image
//! — which live in [`suite`].
//!
//! The suite is graded by `cargo run --release --example eval` against a real
//! llama-server. Everything in these modules is pure, so the scoring itself is
//! covered by ordinary unit tests that run with the GPU asleep.

pub mod expect;
pub mod format;
pub mod suite;

use format::Fault;

/// What one document scored.
#[derive(Debug, Clone)]
pub struct Score {
    pub name: &'static str,
    pub recall: expect::Recall,
    /// Things that are wrong and must not be. These fail the document.
    pub faults: Vec<Fault>,
    /// Things worth knowing that no prompt change has fixed. Reported every
    /// run, never a failure — see [`grade`].
    pub warnings: Vec<Fault>,
    pub uncertainty: usize,
    /// How many sections the page was cut into.
    pub sections: usize,
    pub characters: usize,
    pub seconds: f64,
}

impl Score {
    /// A document passes when it is well-formed and recalls enough of the page.
    ///
    /// Not 100%: an anchor is a phrase a person read off an image of
    /// handwriting, and a few of them are genuinely ambiguous. Demanding
    /// perfection would make the suite fail on the anchors rather than on the
    /// transcription. Formatting has no such excuse and must be clean.
    pub fn passed(&self) -> bool {
        self.faults.is_empty() && self.recall.fraction() >= self.floor()
    }

    /// The recall a document has to clear.
    pub fn floor(&self) -> f64 {
        0.85
    }
}

/// Grade one transcription.
///
/// `expect_uncertainty` says this page is messy enough that a model reading it
/// honestly ought to have doubted something. Silent guessing is the failure
/// that costs a reader most — a wrong word shaped like a right one is worse
/// than a word marked unsure — and no other check here can see it: the output
/// is well-formed and the anchors it did read still hit.
///
/// It is a **warning** rather than a fault, and the reason is measured rather
/// than assumed. Qwen3.6 will not emit `[unclear: ...]` on this server with
/// thinking disabled, and thinking cannot be enabled: on a full section the model
/// spends every token reasoning and returns empty content. Several wordings of
/// the instruction moved the count between zero and one. A check that no change
/// can turn green stops being a signal and starts being noise that hides the
/// ones that matter, so it is reported every run and gates nothing.
pub fn grade(
    name: &'static str,
    transcript: &str,
    anchors: &[&'static str],
    expect_uncertainty: bool,
    sections: usize,
    seconds: f64,
) -> Score {
    let uncertainty = format::uncertainty(transcript);
    let mut warnings = Vec::new();

    if expect_uncertainty && uncertainty == 0 && !transcript.trim().is_empty() {
        warnings.push(Fault {
            rule: "marks its doubts",
            detail: "a page of cursive came back with no [unclear: ...] at all, \
                     so any misreading in it is silent"
                .into(),
        });
    }

    Score {
        name,
        recall: expect::recall(transcript, anchors),
        faults: format::faults(transcript),
        warnings,
        uncertainty,
        sections,
        characters: transcript.chars().count(),
        seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(transcript: &str, anchors: &[&'static str]) -> Score {
        grade("test", transcript, anchors, false, 1, 0.0)
    }

    fn score_messy(transcript: &str, anchors: &[&'static str]) -> Score {
        grade("test", transcript, anchors, true, 1, 0.0)
    }

    #[test]
    fn a_messy_page_read_with_no_doubt_at_all_is_warned_about_but_not_failed() {
        let confident = score_messy("- WealthSpire\n- TopBuild", &["WealthSpire", "TopBuild"]);
        assert!(confident
            .warnings
            .iter()
            .any(|w| w.rule == "marks its doubts"));
        assert!(confident.passed());
    }

    #[test]
    fn a_messy_page_that_marked_something_is_not_warned_about() {
        let honest = score_messy(
            "- WealthSpire\n- [unclear: TopBuild]",
            &["WealthSpire", "TopBuild"],
        );
        assert!(honest.warnings.is_empty());
        assert!(honest.passed());
    }

    #[test]
    fn a_neat_page_is_not_expected_to_doubt_itself() {
        assert!(score("- WealthSpire", &["WealthSpire"]).warnings.is_empty());
    }

    #[test]
    fn a_page_that_read_as_nothing_is_a_fault_not_merely_a_warning() {
        let empty = score_messy("", &["WealthSpire"]);
        assert!(!empty.passed());
        assert!(!empty.faults.is_empty());
    }

    #[test]
    fn a_clean_transcript_that_read_the_page_passes() {
        let score = score(
            "## Current Needs\n\n- WealthSpire\n- TopBuild\n- Eaton",
            &["WealthSpire", "TopBuild", "Eaton"],
        );
        assert!(score.passed());
        assert_eq!(score.recall.fraction(), 1.0);
    }

    #[test]
    fn a_transcript_that_missed_most_of_the_page_fails_even_when_tidy() {
        let score = score("- WealthSpire", &["WealthSpire", "TopBuild", "Eaton"]);
        assert!(score.faults.is_empty());
        assert!(!score.passed());
    }

    #[test]
    fn a_word_perfect_transcript_in_a_code_fence_still_fails() {
        let score = score(
            "```\n- WealthSpire\n- TopBuild\n```",
            &["WealthSpire", "TopBuild"],
        );
        assert_eq!(score.recall.fraction(), 1.0);
        assert!(!score.passed());
    }

    #[test]
    fn one_ambiguous_anchor_out_of_ten_does_not_fail_a_document() {
        let anchors: Vec<&'static str> = vec![
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliett",
        ];
        // Nine of ten.
        let transcript = "alpha bravo charlie delta echo foxtrot golf hotel india";
        let score = score(transcript, &anchors);
        assert_eq!(score.recall.found(), 9);
        assert!(score.passed());
    }
}
