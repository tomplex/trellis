// trellis/src/tui/job.rs — background work with a shared progress indicator.
//
// The event loop is single-threaded: slow work in handle_event/tick/a constructor
// blocks both repaint and input. A Job moves that work to a thread and hands
// results back over a channel for the main thread to apply.
//
// The sqlite Connection never crosses the boundary — it is !Sync and the db runs
// in rollback-journal mode, so a second write connection would risk SQLITE_BUSY.
// Workers shell out and walk the filesystem; the main thread commits.

use std::sync::mpsc;
use std::time::Instant;

use ratatui::prelude::*;

use super::theme;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_MS: u128 = 80;

pub enum JobMsg<T> {
    /// One unit of work finished. The payload is whatever the main thread needs
    /// to commit the result.
    Item(T),
    /// Advisory label for the indicator — what the worker is on right now.
    Status(String),
    /// Worker finished cleanly. Nothing follows it.
    Done,
}

pub struct Job<T> {
    rx: mpsc::Receiver<JobMsg<T>>,
    label: String,
    done: usize,
    /// None renders a bare spinner instead of a count.
    total: Option<usize>,
    current: Option<String>,
    started: Instant,
    finished: bool,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn<F>(label: impl Into<String>, total: Option<usize>, work: F) -> Self
    where
        F: FnOnce(mpsc::Sender<JobMsg<T>>) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || work(tx));
        Self {
            rx,
            label: label.into(),
            done: 0,
            total,
            current: None,
            started: Instant::now(),
            finished: false,
        }
    }
}

impl<T> Job<T> {
    /// Non-blocking. Drains everything queued, advances the counters and returns
    /// the payloads for the caller to apply. Call from `tick()`.
    pub fn drain(&mut self) -> Vec<T> {
        let mut items = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(JobMsg::Item(item)) => {
                    self.done += 1;
                    items.push(item);
                }
                Ok(JobMsg::Status(s)) => self.current = Some(s),
                Ok(JobMsg::Done) => {
                    self.finished = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                // Sender dropped without a Done — worker panicked. try_recv only
                // reports this once the queue is drained, so nothing is lost.
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        items
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Units completed so far — also the index of the in-flight unit when the
    /// worker processes its queue in order.
    pub fn done(&self) -> usize {
        self.done
    }

    pub fn indicator_line(&self) -> Line<'static> {
        let frame = SPINNER[(self.started.elapsed().as_millis() / FRAME_MS) as usize % SPINNER.len()];

        let mut spans = vec![
            Span::styled(format!(" {} ", frame), Style::default().fg(theme::ACCENT)),
            Span::styled(self.label.clone(), Style::default().fg(theme::TEXT)),
        ];
        if let Some(total) = self.total {
            spans.push(Span::styled(
                format!(" {}/{}", self.done, total),
                Style::default().fg(theme::ACCENT),
            ));
        }
        if let Some(ref current) = self.current {
            spans.push(Span::styled(
                format!(" — {}", current),
                Style::default().fg(theme::TEXT_DIM),
            ));
        }
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain until the worker reports Done, collecting everything it sent.
    fn drain_to_completion<T>(job: &mut Job<T>) -> Vec<T> {
        let mut all = Vec::new();
        while !job.finished() {
            all.extend(job.drain());
        }
        all.extend(job.drain());
        all
    }

    #[test]
    fn delivers_every_item_in_order_and_counts_them() {
        let mut job = Job::spawn("Deleting", Some(3), |tx| {
            for i in 0..3 {
                tx.send(JobMsg::Status(format!("item-{}", i))).unwrap();
                tx.send(JobMsg::Item(i)).unwrap();
            }
            tx.send(JobMsg::Done).unwrap();
        });

        let items = drain_to_completion(&mut job);

        assert_eq!(items, vec![0, 1, 2]);
        assert_eq!(job.done(), items.len());
        assert!(job.finished());
    }

    #[test]
    fn status_messages_do_not_count_as_completed_units() {
        let mut job: Job<()> = Job::spawn("Working", None, |tx| {
            tx.send(JobMsg::Status("only a label".to_string())).unwrap();
            tx.send(JobMsg::Done).unwrap();
        });

        let items = drain_to_completion(&mut job);

        assert!(items.is_empty());
        assert_eq!(job.done(), 0);
    }

    #[test]
    fn finishes_when_worker_drops_sender_without_done() {
        let mut job = Job::spawn("Working", Some(1), |tx| {
            tx.send(JobMsg::Item(7)).unwrap();
            // No Done — simulates a worker that panicked partway.
        });

        let items = drain_to_completion(&mut job);

        assert_eq!(items, vec![7], "buffered items survive a disconnect");
        assert!(job.finished());
    }

    #[test]
    fn spawn_returns_while_the_worker_is_still_running() {
        // The worker parks on this gate, so "spawn already returned" is a fact,
        // not a race. Running the closure inline would never reach the asserts.
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let mut job = Job::spawn("Blocked", Some(1), move |tx| {
            gate_rx.recv().unwrap();
            tx.send(JobMsg::Item(1)).unwrap();
            tx.send(JobMsg::Done).unwrap();
        });

        assert!(job.drain().is_empty(), "no work can have landed yet");
        assert!(!job.finished());

        gate_tx.send(()).unwrap();
        assert_eq!(drain_to_completion(&mut job), vec![1]);
    }

    #[test]
    fn drain_is_non_blocking_before_any_work_arrives() {
        // A worker that never sends must not stall the caller.
        let (tx, rx) = mpsc::channel::<JobMsg<()>>();
        let mut job = Job {
            rx,
            label: "Idle".to_string(),
            done: 0,
            total: None,
            current: None,
            started: Instant::now(),
            finished: false,
        };

        assert!(job.drain().is_empty());
        assert!(!job.finished());
        drop(tx);
    }
}
