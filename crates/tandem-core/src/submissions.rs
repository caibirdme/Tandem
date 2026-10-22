use std::{collections::HashMap, sync::Mutex};

use tokio::sync::watch;

use crate::{Error, Result};

struct PendingSubmission {
    cancel: watch::Sender<bool>,
    preparing: bool,
}

/// Submissions remain cancellable until their message is committed to history.
#[derive(Default)]
pub(crate) struct Submissions(Mutex<HashMap<String, PendingSubmission>>);

impl Submissions {
    pub fn begin(&self, id: &str) -> Result<watch::Receiver<bool>> {
        let mut pending = self.0.lock().map_err(|_| Error::SessionStatePoisoned)?;
        if pending.contains_key(id) {
            return Err(Error::ConversationRunning);
        }
        let (cancel, receiver) = watch::channel(false);
        pending.insert(
            id.into(),
            PendingSubmission {
                cancel,
                preparing: false,
            },
        );
        Ok(receiver)
    }

    pub fn prepare(&self, id: &str) -> Result<()> {
        if let Some(pending) = self
            .0
            .lock()
            .map_err(|_| Error::SessionStatePoisoned)?
            .get_mut(id)
        {
            pending.preparing = true;
        }
        Ok(())
    }

    pub fn finish(&self, id: &str) -> Result<(bool, bool)> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::SessionStatePoisoned)?
            .remove(id)
            .map_or((false, false), |pending| {
                (pending.preparing, *pending.cancel.borrow())
            }))
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        if let Some(pending) = self
            .0
            .lock()
            .map_err(|_| Error::SessionStatePoisoned)?
            .get(id)
        {
            pending.cancel.send_replace(true);
        }
        Ok(())
    }

    pub fn preparing(&self) -> Result<Vec<String>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::SessionStatePoisoned)?
            .iter()
            .filter(|(_, pending)| pending.preparing)
            .map(|(id, _)| id.clone())
            .collect())
    }

    pub fn pending(&self) -> Result<Vec<String>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::SessionStatePoisoned)?
            .keys()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_scoped_to_one_attempt_and_commit_removes_it() -> Result<()> {
        let submissions = Submissions::default();
        let first = submissions.begin("one")?;
        let other = submissions.begin("two")?;
        assert!(matches!(
            submissions.begin("one"),
            Err(Error::ConversationRunning)
        ));
        submissions.prepare("one")?;
        assert_eq!(submissions.preparing()?, ["one"]);
        submissions.cancel("one")?;
        assert!(*first.borrow());
        assert!(!*other.borrow());
        assert_eq!(submissions.finish("one")?, (true, true));
        let retry = submissions.begin("one")?;
        assert!(!*retry.borrow());
        assert_eq!(submissions.finish("one")?, (false, false));
        submissions.cancel("one")?;
        assert!(!*retry.borrow());
        assert_eq!(submissions.preparing()?, Vec::<String>::new());
        Ok(())
    }
}
