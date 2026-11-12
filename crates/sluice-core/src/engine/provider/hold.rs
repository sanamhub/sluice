//! The wait a one-download sender keeps after its last byte, so a receiver that is still installing
//! what it received can ask for a receipt before the endpoint closes.

use std::collections::BTreeSet;

use iroh_blobs::Hash;

use super::Provider;

impl Provider {
    /// Keeps answering receipt requests after the last byte, for a receiver that is still installing
    /// what it received and asks before it closes. With `until_closed` the hold lasts while a
    /// receiver's content connection is open and no receipt has been delivered; without it, only
    /// while an answer already decided is being written. Either ends after ten seconds: a peer that
    /// keeps its connection open cannot keep the sender alive.
    pub(super) async fn hold_receipts(
        &mut self,
        until_closed: bool,
        opened: &mut BTreeSet<u64>,
        roots: &BTreeSet<Hash>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) {
        if !self.receipts.enabled() {
            return;
        }
        let deadline = tokio::time::sleep(crate::engine::receipts::HOLD);
        tokio::pin!(deadline);
        while self.receipts.pending() > 0
            || (until_closed && !opened.is_empty() && self.receipts.answered() == 0)
        {
            tokio::select! {
                _ = &mut deadline => break,
                _ = self.receipts.wait_changed() => {}
                reported = self.receivers.recv() => {
                    let Some(reported) = reported else { break };
                    Self::track(&reported, opened);
                    self.report(reported, false, roots, events);
                }
            }
        }
    }
}
