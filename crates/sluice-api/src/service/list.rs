//! Listing the entries under a path below a root.

use sluice_core::manifest::{EntryKind, list_folder};

use super::Service;
use crate::error::ApiError;
use crate::proto;
use crate::roots::Use;

/// How many entries one answer lists. A folder with more is cut, and the answer says so.
pub const MAX_ENTRIES: usize = 10_000;

impl Service {
    /// The entries directly under a path below a root: name, kind and size, folders first. The path
    /// is opened as every path under a root is, and the listing is read from the folder's handle. A
    /// link is listed and never followed. No file content is in the answer.
    pub async fn list_entries(
        &self,
        request: proto::ListEntriesRequest,
    ) -> Result<proto::ListEntriesResponse, ApiError> {
        self.refuse_without_roots()?;
        let opened = self.inner.roots.resolve(request.path.as_ref(), Use::Read)?;
        if opened.is_file {
            return Err(ApiError::Invalid(
                "that path is a file, and only a folder has entries".to_string(),
            ));
        }
        let handle = opened
            .handle
            .try_clone()
            .map_err(|e| ApiError::Refused(format!("cannot read the folder: {}", e.kind())))?;
        let listing = tokio::task::spawn_blocking(move || list_folder(handle, MAX_ENTRIES))
            .await
            .map_err(|_| ApiError::Failed("the listing did not finish".to_string()))?
            .map_err(|e| ApiError::Failed(format!("cannot list the folder: {}", e.kind())))?;
        Ok(proto::ListEntriesResponse {
            entries: listing
                .entries
                .into_iter()
                .map(|entry| proto::DirEntry {
                    name: Some(proto::PeerName { text: entry.name }),
                    kind: match entry.kind {
                        EntryKind::File => proto::EntryKind::File,
                        EntryKind::Folder => proto::EntryKind::Folder,
                        EntryKind::Link => proto::EntryKind::Link,
                    } as i32,
                    size: entry.size,
                })
                .collect(),
            truncated: listing.truncated,
        })
    }
}

#[cfg(test)]
mod tests;
