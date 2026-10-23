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

    /// Reserve an idle session, but leave an existing turn intact.
    pub(crate) fn begin_steering(&self) -> Result<bool> {
        let mut result = Ok(false);
        self.phase.send_if_modified(|phase| match phase {
            Phase::Idle => {
                *phase = Phase::Running;
                result = Ok(true);
                true
            }
            Phase::Running => false,
            Phase::Disconnected => {
                result = Err(Error::Disconnected);
                false
            }
        });
        result
    }

    /// The runtime may start another turn after an immediate message falls back
    /// to its queue. Never revive a disconnected session on a late event.
    pub(crate) fn observe_running(&self) {
        let _ = self.begin_steering();
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

    #[test]
    fn steering_and_late_turn_start_preserve_lifecycle_boundaries() -> Result<()> {
        let lifecycle = SessionLifecycle::new();
        assert!(lifecycle.begin_steering()?);
        assert!(!lifecycle.begin_steering()?);
        lifecycle.finish_turn();
        lifecycle.observe_running();
        assert!(lifecycle.is_running());
        assert!(matches!(lifecycle.begin_turn(), Err(Error::Busy)));
        lifecycle.disconnect();
        lifecycle.observe_running();
        assert!(!lifecycle.is_connected());
        assert!(matches!(
            lifecycle.begin_steering(),
            Err(Error::Disconnected)
        ));
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
