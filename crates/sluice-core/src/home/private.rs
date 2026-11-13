//! The small files the home keeps for a running process to hand something to another command.

use std::fs;

use super::{Home, write_atomic_secret};
use crate::Error;

impl Home {
    /// Writes a small file in the home that only this user can read (mode 0600, or an access list of
    /// its own on Windows), replaced whole or not at all. The daemon hands its port and key to
    /// `sluice ui` with it.
    pub fn write_private(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        write_atomic_secret(&self.path, name, bytes)
    }

    /// Removes a file written with [`Home::write_private`], if it is there.
    pub fn remove_private(&self, name: &str) {
        let _ = fs::remove_file(self.path.join(name));
    }
}
