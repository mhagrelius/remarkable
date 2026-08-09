//! Driving a [`Job`] against a live server.
//!
//! The loop is a chain of callbacks on the main loop rather than a loop: each
//! section's answer schedules the next section's request. That keeps the whole thing
//! on the thread that owns the widgets — the view is updated directly from the
//! callback, with no channel and no locking — and makes cancellation a single
//! `gio::Cancellable` that the Stop button and the window's close both hold.
//!
//! A section that fails does not end the run. Losing one section of a twelve-section
//! notebook to a hiccup, and with it the eleven that already succeeded, is the
//! wrong trade; the lapse is recorded and the next section is asked for.

use std::cell::RefCell;
use std::rc::Rc;

use gio::prelude::*;

use crate::model::job::{Job, Lapse};
use crate::model::raster::Page;
use crate::model::wire::{ChatRequest, DataUrl};

use super::client::{Client, ClientError};

/// A run in progress. Dropping the last reference does not stop it; cancel it.
pub struct Run {
    client: Rc<Client>,
    pages: Vec<Page>,
    model: Option<String>,
    job: RefCell<Job>,
    cancellable: gio::Cancellable,
    on_progress: Box<dyn Fn(&Job)>,
    /// `FnOnce` behind a `RefCell` so the several places a run can end — the
    /// last section, a cancel, a cancelled request in flight — can each take it
    /// and call it exactly once.
    on_done: RefCell<Option<Finish>>,
}

/// What runs when a run ends.
type Finish = Box<dyn FnOnce(&Job, Outcome)>;

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every section was asked for. Some may have lapsed; [`Job::lapses`] says.
    Complete,
    /// Stopped part-way. What was read is kept.
    Cancelled,
}

impl Run {
    /// Start reading. `pages` must be in the same order the job was planned in.
    ///
    /// `on_progress` runs after every section, with the job as it now stands, so
    /// the window can show the transcript growing. `on_done` runs exactly once.
    pub fn start(
        client: Rc<Client>,
        pages: Vec<Page>,
        job: Job,
        model: Option<String>,
        on_progress: impl Fn(&Job) + 'static,
        on_done: impl FnOnce(&Job, Outcome) + 'static,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            client,
            pages,
            model,
            job: RefCell::new(job),
            cancellable: gio::Cancellable::new(),
            on_progress: Box::new(on_progress),
            on_done: RefCell::new(Some(Box::new(on_done))),
        });
        run.step();
        run
    }

    /// Abandon the run. The section in flight is dropped; everything read before
    /// it is kept, and `on_done` reports [`Outcome::Cancelled`].
    pub fn cancel(&self) {
        self.cancellable.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellable.is_cancelled()
    }

    /// A snapshot of the job, for the view to read.
    pub fn job(&self) -> std::cell::Ref<'_, Job> {
        self.job.borrow()
    }

    fn step(self: &Rc<Self>) {
        if self.cancellable.is_cancelled() {
            self.finish(Outcome::Cancelled);
            return;
        }

        // The borrow ends before anything is sent: the callback below borrows
        // the job again, and on the error paths it does so synchronously.
        let Some(step) = self.job.borrow().next_step() else {
            self.finish(Outcome::Complete);
            return;
        };

        let png = match self.pages[step.page].section_png(&step.section) {
            Ok(png) => png,
            Err(error) => {
                // A section that cannot be cut is this section's failure, not the
                // document's; the rest of the page may still read.
                self.record(Lapse::Failed(error.to_string()), None);
                self.step();
                return;
            }
        };

        let request =
            ChatRequest::transcribe(self.model.clone(), &step.prompt, &DataUrl::png(&png));

        let run = Rc::clone(self);
        let sent = self.client.transcribe(&request, move |result| {
            match result {
                Ok(completion) if completion.truncated => {
                    run.record(Lapse::Truncated, Some(&completion.text));
                }
                Ok(completion) => {
                    run.job.borrow_mut().accept(&completion.text);
                    (run.on_progress)(&run.job.borrow());
                }
                Err(ClientError::Cancelled) => {
                    run.finish(Outcome::Cancelled);
                    return;
                }
                Err(error) => run.record(Lapse::Failed(error.to_string()), None),
            }
            run.step();
        });

        // One cancellable for the run: cancelling it cancels whatever is in
        // flight, now and for every section after this one.
        self.cancellable.connect_cancelled(move |_| sent.cancel());
    }

    fn record(&self, lapse: Lapse, partial: Option<&str>) {
        self.job.borrow_mut().accept_lapse(lapse, partial);
        (self.on_progress)(&self.job.borrow());
    }

    fn finish(&self, outcome: Outcome) {
        if let Some(done) = self.on_done.borrow_mut().take() {
            done(&self.job.borrow(), outcome);
        }
    }
}
