//! Session transitions are independent of the lossy UI event broadcast.

use tokio::sync::watch;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Running,
    Disconnected,
}

pub(crate) struct SessionLifecycle {
    phase: watch::Sender<Phase>,
}

impl SessionLifecycle {
    pub(crate) fn new() -> Self {
        Self {
            phase: watch::channel(Phase::Idle).0,
        }
    }

    pub(crate) fn begin_turn(&self) -> Result<()> {
        let mut result = Ok(());
        self.phase.send_if_modified(|phase| match phase {
            Phase::Idle => {
                *phase = Phase::Running;
                true
            }
            Phase::Running => {
                result = Err(Error::Busy);
                false
            }
            Phase::Disconnected => {
                result = Err(Error::Disconnected);
                false
            }
        });
        result
    }

    pub(crate) fn finish_turn(&self) {
        self.phase.send_if_modified(|phase| {
            if *phase == Phase::Running {
                *phase = Phase::Idle;
                true
            } else {
                false
            }
        });
    }

    /// Returns whether disconnection interrupted an active turn.
    pub(crate) fn disconnect(&self) -> bool {
        self.phase.send_replace(Phase::Disconnected) == Phase::Running
    }

    pub(crate) fn is_connected(&self) -> bool {
        *self.phase.borrow() != Phase::Disconnected
    }

    pub(crate) fn is_running(&self) -> bool {
        *self.phase.borrow() == Phase::Running
    }

    pub(crate) async fn wait_until_idle(&self) -> Result<()> {
        let mut phase = self.phase.subscribe();
        loop {
            match *phase.borrow_and_update() {
                Phase::Idle => return Ok(()),
                Phase::Disconnected => return Err(Error::Disconnected),
                Phase::Running => {}
            }
            phase.changed().await.map_err(|_| Error::Disconnected)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disconnected_session_cannot_be_reopened_by_a_late_idle_event() -> Result<()> {
        let lifecycle = SessionLifecycle::new();
        lifecycle.begin_turn()?;
        assert!(matches!(lifecycle.begin_turn(), Err(Error::Busy)));
        assert!(lifecycle.disconnect());
        lifecycle.finish_turn();
        assert!(!lifecycle.is_connected());
        assert!(matches!(lifecycle.begin_turn(), Err(Error::Disconnected)));
        assert!(!lifecycle.disconnect());
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_wait_distinguishes_idle_from_disconnection() -> Result<()> {
        for disconnect in [false, true] {
            let lifecycle = SessionLifecycle::new();
            lifecycle.begin_turn()?;
            let (result, ()) = tokio::join!(lifecycle.wait_until_idle(), async {
                if disconnect {
                    lifecycle.disconnect();
                } else {
                    lifecycle.finish_turn();
                }
            });
            if disconnect {
                assert!(matches!(result, Err(Error::Disconnected)));
            } else {
                result?;
                lifecycle.begin_turn()?;
            }
        }
        Ok(())
    }
}
