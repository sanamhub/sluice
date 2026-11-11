//! The wait a one-download sender keeps after its last byte, so a receiver that is still installing
//! what it received can ask for a receipt before the endpoint closes.

use std::collections::BTreeSet;

use iroh_blobs::Hash;

use super::Provider;

impl Provider {
    /// Keeps answering receipt requests after the last byte, for a receiver that is still installing
    /// what it received and asks before it closes. The hold ends when the receiver's content
    /// connection closes, when a receipt has been written, or after ten seconds, whichever is
    /// first: a peer that keeps its connection open cannot keep the sender alive.
    pub(super) async fn hold_receipts(
        &mut self,
        opened: &mut BTreeSet<u64>,
        roots: &BTreeSet<Hash>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) {
        if !self.receipts.enabled() {
            return;
        }
        let deadline = tokio::time::sleep(crate::engine::receipts::HOLD);
        tokio::pin!(deadline);
        while !opened.is_empty() && self.receipts.answered() == 0 {
            tokio::select! {
                _ = &mut deadline => break,
                _ = self.receipts.wait_answered() => break,
                reported = self.receivers.recv() => {
                    let Some(reported) = reported else { break };
                    Self::track(&reported, opened);
                    self.report(reported, false, roots, events);
                }
            }
        }
    }
}
