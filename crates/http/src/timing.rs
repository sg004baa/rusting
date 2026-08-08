//! Timing accumulation and live phase-event delivery.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::types::{Phase, PhaseEvent, Timings};

/// Records supported request phases and mirrors each state transition to the
/// optional live progress channel. A closed receiver never cancels a send.
pub(crate) struct Recorder {
    timings: Timings,
    progress: Option<UnboundedSender<PhaseEvent>>,
}

impl Recorder {
    pub(crate) fn new(progress: Option<UnboundedSender<PhaseEvent>>) -> Self {
        Self {
            timings: Timings::default(),
            progress,
        }
    }

    pub(crate) fn start(&mut self, phase: Phase) {
        self.publish(PhaseEvent::Started(phase));
    }

    pub(crate) fn complete(&mut self, phase: Phase, started: Instant) {
        self.publish(PhaseEvent::Completed(phase, started.elapsed()));
    }

    pub(crate) fn fail(&mut self, phase: Phase) {
        self.publish(PhaseEvent::Failed(phase));
    }

    pub(crate) fn finish(mut self, total: Duration) -> Timings {
        self.timings.total = Some(total);
        self.timings
    }

    fn publish(&mut self, event: PhaseEvent) {
        self.timings.apply(event);
        if let Some(progress) = &self.progress {
            let _receiver_may_be_closed = progress.send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PhaseOutcome;

    #[test]
    fn closed_progress_receiver_does_not_prevent_recording() {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        drop(receiver);
        let mut recorder = Recorder::new(Some(sender));
        let started = Instant::now();
        recorder.start(Phase::Download);
        recorder.complete(Phase::Download, started);
        let timings = recorder.finish(Duration::from_millis(5));
        assert!(matches!(
            timings.outcome(Phase::Download),
            PhaseOutcome::Completed(_)
        ));
        assert_eq!(timings.total, Some(Duration::from_millis(5)));
    }
}
