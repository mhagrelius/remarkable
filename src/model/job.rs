//! Reading a document, one section at a time.
//!
//! A [`Job`] holds no image, opens no socket and knows nothing about GTK. It
//! says which section to read next and what to ask about it, takes the answer
//! back, and at the end hands over a [`Transcript`]. The caller owns the
//! pixels and the network.
//!
//! That split is what lets the window and `examples/eval` drive the identical
//! pipeline — one on the GLib main loop with a soup session, the other in a
//! plain loop against recorded fixtures or a real server — without either
//! reimplementing the order of operations.

use super::prompt;
use super::sections::{self, Layout, Section};
use super::text;
use super::transcript::{PageText, Transcript};

/// Turn a section's raw answer into the text that is kept.
///
/// The model writes its doubts as a trailing `---UNCERTAIN---` list rather than
/// as inline markers — see [`text::FOOTER_MARKER`] for why that is the only
/// shape it will produce. Here the list is taken off, applied to the words it
/// names, and thrown away, so nothing downstream has to know the footer ever
/// existed: what it sees is a transcript with `[unclear: ...]` in it.
fn transcribed(raw: &str) -> String {
    let (body, doubts) = text::split_footer(raw);
    text::mark_doubts(&text::clean(body), &doubts)
}

/// A page waiting to be read, reduced to the shape [`Job`] needs: how tall it
/// is and where the writing is not.
pub struct PagePlan {
    pub number: usize,
    pub sections: Vec<Section>,
}

impl PagePlan {
    /// Work out where to cut a page from its row profile and its width.
    pub fn from_profile(number: usize, profile: &[u8], width: u32, layout: &Layout) -> Self {
        Self {
            number,
            sections: sections::split(profile, width, layout),
        }
    }
}

/// The next thing to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Index into the pages the caller planned, not the page number.
    pub page: usize,
    pub section: Section,
    pub prompt: String,
}

/// How far along, for a progress bar and a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub page: usize,
    pub pages: usize,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.done as f64 / self.total as f64
    }
}

/// Why a section produced no text. Kept rather than thrown so a document with one
/// bad section still saves, with the gap marked where it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lapse {
    /// The server could not be reached, or refused.
    Failed(String),
    /// The model stopped at the token ceiling. What came back is kept; the note
    /// records that the tail is missing.
    Truncated,
}

pub struct Job {
    pages: Vec<PagePlan>,
    /// One entry per section, in the order the sections are read.
    answers: Vec<Option<String>>,
    lapses: Vec<(usize, Lapse)>,
    at: usize,
}

impl Job {
    pub fn new(pages: Vec<PagePlan>) -> Self {
        let sections = pages.iter().map(|page| page.sections.len()).sum();
        Self {
            pages,
            answers: vec![None; sections],
            lapses: Vec::new(),
            at: 0,
        }
    }

    /// Plan a document from its pages' row profiles and widths.
    pub fn from_profiles(profiles: &[(Vec<u8>, u32)], layout: &Layout) -> Self {
        Self::new(
            profiles
                .iter()
                .enumerate()
                .map(|(index, (profile, width))| {
                    PagePlan::from_profile(index + 1, profile, *width, layout)
                })
                .collect(),
        )
    }

    pub fn total_sections(&self) -> usize {
        self.answers.len()
    }

    pub fn is_finished(&self) -> bool {
        self.at >= self.answers.len()
    }

    /// What to send next, or `None` when the document is read.
    pub fn next_step(&self) -> Option<Step> {
        let (page, within) = self.locate(self.at)?;
        let section = self.pages[page].sections[within];
        Some(Step {
            page,
            section,
            prompt: prompt::for_section(&section, self.previous_text(page, within)),
        })
    }

    /// Record what came back, and move on.
    pub fn accept(&mut self, text: &str) {
        if self.at < self.answers.len() {
            self.answers[self.at] = Some(transcribed(text));
            self.at += 1;
        }
    }

    /// Record that a section did not produce usable text, and move on.
    ///
    /// A truncated section keeps whatever arrived — half a section is better than
    /// none, and the merge will still find its seam with the section above. Its
    /// footer is gone with the tail, so it contributes no marks.
    pub fn accept_lapse(&mut self, lapse: Lapse, partial: Option<&str>) {
        if self.at >= self.answers.len() {
            return;
        }
        self.answers[self.at] = partial.map(transcribed);
        self.lapses.push((self.at, lapse));
        self.at += 1;
    }

    pub fn progress(&self) -> Progress {
        let (page, _) = self
            .locate(self.at.min(self.answers.len().saturating_sub(1)))
            .unwrap_or((self.pages.len().saturating_sub(1), 0));
        Progress {
            done: self.at,
            total: self.answers.len(),
            page: page + 1,
            pages: self.pages.len(),
        }
    }

    /// Everything read so far, merged. Safe to call mid-run — the window shows
    /// it after every section so a long document is visibly working.
    pub fn text_so_far(&self) -> String {
        self.pages
            .iter()
            .enumerate()
            .map(|(index, page)| self.merged_page(index, page))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// What went wrong, in the order it went wrong.
    pub fn lapses(&self) -> &[(usize, Lapse)] {
        &self.lapses
    }

    pub fn finish(&self, source: &str, model: &str, processed: &str) -> Transcript {
        Transcript {
            source: source.to_string(),
            model: model.to_string(),
            processed: processed.to_string(),
            pages: self
                .pages
                .iter()
                .enumerate()
                .map(|(index, page)| PageText {
                    number: page.number,
                    text: self.merged_page(index, page),
                    sections: page.sections.len(),
                })
                .collect(),
        }
    }

    /// Which page and which of its sections the nth section overall is.
    fn locate(&self, nth: usize) -> Option<(usize, usize)> {
        let mut seen = 0;
        for (index, page) in self.pages.iter().enumerate() {
            if nth < seen + page.sections.len() {
                return Some((index, nth - seen));
            }
            seen += page.sections.len();
        }
        None
    }

    /// Where a page's sections start in `answers`.
    fn offset_of(&self, page: usize) -> usize {
        self.pages[..page].iter().map(|p| p.sections.len()).sum()
    }

    /// The transcript of the section above, for context. Only within a page — the
    /// bottom of one page does not continue into the top of the next.
    fn previous_text(&self, page: usize, within: usize) -> Option<&str> {
        let previous = within.checked_sub(1)?;
        self.answers[self.offset_of(page) + previous].as_deref()
    }

    fn merged_page(&self, index: usize, page: &PagePlan) -> String {
        let from = self.offset_of(index);
        let sections: Vec<String> = self.answers[from..from + page.sections.len()]
            .iter()
            .flatten()
            .cloned()
            .collect();
        super::merge::merge(&sections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: u32 = 1620;

    /// A profile tall enough to need `sections` sections, with clean gaps to cut in.
    fn tall_page(rows: usize) -> Vec<u8> {
        let mut profile = Vec::with_capacity(rows);
        while profile.len() < rows {
            profile.extend(std::iter::repeat_n(120u8, 40));
            profile.extend(std::iter::repeat_n(252u8, 30));
        }
        profile.truncate(rows);
        profile
    }

    fn short_page() -> Vec<u8> {
        tall_page(1000)
    }

    /// Drive a job to completion, answering every step with `answer`.
    fn run(job: &mut Job, mut answer: impl FnMut(&Step) -> String) {
        while let Some(step) = job.next_step() {
            let text = answer(&step);
            job.accept(&text);
        }
    }

    #[test]
    fn a_short_page_is_one_section_and_one_step() {
        let mut job = Job::from_profiles(&[(short_page(), WIDTH)], &Layout::default());
        assert_eq!(job.total_sections(), 1);

        let step = job.next_step().expect("a step");
        assert_eq!(step.page, 0);
        assert_eq!(step.section.total, 1);
        // A page read whole gets no "where this image sits" note.
        assert!(!step.prompt.contains("Where this image sits"));

        job.accept("- a bullet");
        assert!(job.is_finished());
        assert!(job.next_step().is_none());
    }

    #[test]
    fn a_tall_page_is_walked_section_by_section_in_order() {
        let mut job = Job::from_profiles(&[(tall_page(9000), WIDTH)], &Layout::default());
        assert!(
            job.total_sections() >= 6,
            "{} sections",
            job.total_sections()
        );

        let mut seen = Vec::new();
        run(&mut job, |step| {
            seen.push(step.section.index);
            format!("section {}", step.section.index)
        });

        assert_eq!(seen, (0..job.total_sections()).collect::<Vec<_>>());
        assert!(job.is_finished());
    }

    #[test]
    fn each_section_after_the_first_is_told_what_the_last_one_read() {
        let mut job = Job::from_profiles(&[(tall_page(9000), WIDTH)], &Layout::default());
        let mut prompts = Vec::new();
        run(&mut job, |step| {
            prompts.push(step.prompt.clone());
            format!("the end of section {}", step.section.index)
        });

        assert!(!prompts[0].contains("previous section ended"));
        assert!(prompts[1].contains("previous section ended"));
        assert!(prompts[1].contains("the end of section 0"));
        assert!(prompts[2].contains("the end of section 1"));
    }

    #[test]
    fn a_second_page_does_not_continue_from_the_first() {
        let mut job = Job::from_profiles(
            &[(short_page(), WIDTH), (short_page(), WIDTH)],
            &Layout::default(),
        );
        assert_eq!(job.total_sections(), 2);

        job.accept("page one");
        let second = job.next_step().expect("a step");
        assert_eq!(second.page, 1);
        assert!(!second.prompt.contains("page one"));
    }

    #[test]
    fn the_sections_of_a_page_are_merged_into_one_transcript() {
        let mut job = Job::from_profiles(&[(tall_page(9000), WIDTH)], &Layout::default());
        let mut n = 0;
        run(&mut job, |_| {
            n += 1;
            // Each section repeats the previous section's last line, as the real
            // overlap does.
            format!("line {}\nline {}", n - 1, n)
        });

        let text = job.text_so_far();
        assert!(text.contains("line 1"));
        // The repeated lines are read once, not once per section.
        assert_eq!(text.lines().filter(|line| *line == "line 2").count(), 1);
    }

    #[test]
    fn progress_counts_sections_and_names_the_page() {
        let mut job = Job::from_profiles(
            &[(short_page(), WIDTH), (tall_page(6000), WIDTH)],
            &Layout::default(),
        );
        let total = job.total_sections();

        assert_eq!(job.progress().done, 0);
        assert_eq!(job.progress().total, total);
        assert_eq!(job.progress().pages, 2);
        assert_eq!(job.progress().page, 1);

        job.accept("first page");
        assert_eq!(job.progress().done, 1);
        assert_eq!(job.progress().page, 2);

        run(&mut job, |_| "more".into());
        assert_eq!(job.progress().done, total);
        assert_eq!(job.progress().fraction(), 1.0);
    }

    #[test]
    fn a_section_that_failed_does_not_stop_the_document() {
        let mut job = Job::from_profiles(&[(tall_page(6000), WIDTH)], &Layout::default());
        job.accept("the first section");
        job.accept_lapse(Lapse::Failed("connection refused".into()), None);
        run(&mut job, |_| "a later section".into());

        assert!(job.is_finished());
        let text = job.text_so_far();
        assert!(text.contains("the first section"));
        assert!(text.contains("a later section"));
        assert_eq!(job.lapses().len(), 1);
        assert!(matches!(job.lapses()[0], (1, Lapse::Failed(_))));
    }

    #[test]
    fn a_truncated_section_keeps_what_arrived() {
        let mut job = Job::from_profiles(&[(tall_page(6000), WIDTH)], &Layout::default());
        job.accept_lapse(Lapse::Truncated, Some("half a section"));
        run(&mut job, |_| "the rest".into());

        assert!(job.text_so_far().contains("half a section"));
        assert_eq!(job.lapses(), &[(0, Lapse::Truncated)]);
    }

    #[test]
    fn the_answers_are_cleaned_before_they_are_kept() {
        let mut job = Job::from_profiles(&[(short_page(), WIDTH)], &Layout::default());
        job.accept("```markdown\n- a bullet\n```");
        assert_eq!(job.text_so_far(), "- a bullet");
    }

    #[test]
    fn the_uncertainty_footer_becomes_markers_and_then_disappears() {
        let mut job = Job::from_profiles(&[(short_page(), WIDTH)], &Layout::default());
        job.accept("- CMOB/Change management\n- Headless runs IT\n\n---UNCERTAIN---\nCMOB\n");

        let text = job.text_so_far();
        assert!(text.contains("[unclear: CMOB]/Change"), "{text}");
        assert!(
            !text.contains("UNCERTAIN"),
            "the footer reached the transcript: {text}"
        );
        assert!(text.contains("Headless runs IT"));
    }

    #[test]
    fn a_footer_is_not_carried_into_the_next_sections_context() {
        // The tail handed to the next section is the transcript, not the raw
        // answer — otherwise the model is shown a list of words and asked to
        // continue from it.
        let mut job = Job::from_profiles(&[(tall_page(9000), WIDTH)], &Layout::default());
        job.accept("- the first section\n\n---UNCERTAIN---\nsection\n");
        let next = job.next_step().expect("a step");
        assert!(!next.prompt.contains("---UNCERTAIN---\nsection"));
        assert!(next.prompt.contains("the first [unclear: section]"));
    }

    #[test]
    fn a_finished_job_becomes_a_transcript() {
        let mut job = Job::from_profiles(
            &[(short_page(), WIDTH), (short_page(), WIDTH)],
            &Layout::default(),
        );
        job.accept("page one");
        job.accept("page two");

        let transcript = job.finish("notes.png", "qwen3.6-27b", "2026-08-02T18:30:00Z");
        assert_eq!(transcript.source, "notes.png");
        assert_eq!(transcript.pages.len(), 2);
        assert_eq!(transcript.pages[0].number, 1);
        assert_eq!(transcript.pages[0].text, "page one");
        assert_eq!(transcript.pages[1].text, "page two");
    }

    #[test]
    fn a_transcript_can_be_taken_mid_run() {
        let mut job = Job::from_profiles(&[(tall_page(9000), WIDTH)], &Layout::default());
        job.accept("what we have so far");
        let transcript = job.finish("notes.png", "qwen3.6-27b", "2026-08-02T18:30:00Z");
        assert!(transcript.pages[0].text.contains("what we have so far"));
    }

    #[test]
    fn accepting_more_than_there_are_sections_does_nothing() {
        let mut job = Job::from_profiles(&[(short_page(), WIDTH)], &Layout::default());
        job.accept("the only section");
        job.accept("nowhere to put this");
        assert_eq!(job.text_so_far(), "the only section");
    }

    #[test]
    fn a_document_with_no_pages_is_finished_before_it_starts() {
        let job = Job::from_profiles(&[], &Layout::default());
        assert!(job.is_finished());
        assert!(job.next_step().is_none());
        assert_eq!(job.progress().fraction(), 1.0);
    }
}
