//! ACSM fulfilment with an existing Adobe/ByteBooks (ADEPT) activation: "ACSM in, book out".
//!
//! The activation and the record of every fulfilment live in a private state directory that the
//! implementation owns; the app never sees them. A fulfilment whose outcome at the server is
//! unknown is never repeated automatically: loans are counted there.
#![forbid(unsafe_code)]

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// The three files of a Calibre "ACSM Input" / libgourou activation export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub device_xml: Vec<u8>,
    pub activation_xml: Vec<u8>,
    /// Exactly 16 bytes.
    pub devicesalt: Vec<u8>,
}

/// Fulfilment of ACSM files. Blocking, and it may contact the publisher's servers: call it from
/// a task.
pub trait Adept: Send + Sync + 'static {
    /// Whether an activation is installed and locally valid. Contacts no server.
    fn status(&self) -> Result<bool, AdeptError>;
    /// Checks and installs an activation. Refused when one is already installed. Contacts no
    /// server.
    fn import_activation(&self, activation: &Activation) -> Result<(), AdeptError>;
    /// Fulfils the ACSM in `acsm`, downloads the book and removes its DRM. Returns the book (an
    /// EPUB archive), which the caller still has to validate. Asking again for the same ACSM
    /// reuses what an earlier attempt saved, and never asks the server twice.
    fn fulfil(&self, acsm: &[u8]) -> Result<Vec<u8>, AdeptError>;
}

impl<T: Adept + ?Sized> Adept for Arc<T> {
    fn status(&self) -> Result<bool, AdeptError> {
        (**self).status()
    }
    fn import_activation(&self, activation: &Activation) -> Result<(), AdeptError> {
        (**self).import_activation(activation)
    }
    fn fulfil(&self, acsm: &[u8]) -> Result<Vec<u8>, AdeptError> {
        (**self).fulfil(acsm)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdeptError(pub String);

impl fmt::Display for AdeptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AdeptError {}

/// Fulfilment for tests: the test says which ACSM gives which book. Contacts nothing.
#[derive(Clone, Default)]
pub struct SimAdept {
    state: Arc<Mutex<SimState>>,
}

/// An ACSM, and the book (or the failure) fulfilling it gives.
type Answer = (Vec<u8>, Result<Vec<u8>, String>);

#[derive(Default)]
struct SimState {
    activation: Option<Activation>,
    books: Vec<Answer>,
    fulfilled: Vec<Vec<u8>>,
}

impl SimAdept {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, SimState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// As if an activation were installed.
    pub fn activated(&self) -> &Self {
        self.state().activation = Some(Activation {
            device_xml: b"<device/>".to_vec(),
            activation_xml: b"<activation/>".to_vec(),
            devicesalt: vec![0; 16],
        });
        self
    }

    /// Fulfilling exactly `acsm` gives `book`.
    pub fn book(&self, acsm: &[u8], book: &[u8]) -> &Self {
        self.state().books.push((acsm.to_vec(), Ok(book.to_vec())));
        self
    }

    /// Fulfilling exactly `acsm` fails with `message`.
    pub fn fails(&self, acsm: &[u8], message: &str) -> &Self {
        self.state()
            .books
            .push((acsm.to_vec(), Err(message.to_string())));
        self
    }

    /// Every ACSM fulfilment was asked for, in order.
    pub fn fulfilled(&self) -> Vec<Vec<u8>> {
        self.state().fulfilled.clone()
    }

    pub fn activation(&self) -> Option<Activation> {
        self.state().activation.clone()
    }
}

impl Adept for SimAdept {
    fn status(&self) -> Result<bool, AdeptError> {
        Ok(self.state().activation.is_some())
    }

    fn import_activation(&self, activation: &Activation) -> Result<(), AdeptError> {
        let mut state = self.state();
        if state.activation.is_some() {
            return Err(AdeptError("An activation is already installed".into()));
        }
        if activation.devicesalt.len() != 16 {
            return Err(AdeptError(
                "devicesalt must contain exactly 16 bytes".into(),
            ));
        }
        state.activation = Some(activation.clone());
        Ok(())
    }

    fn fulfil(&self, acsm: &[u8]) -> Result<Vec<u8>, AdeptError> {
        let mut state = self.state();
        if state.activation.is_none() {
            return Err(AdeptError(
                "No usable activation; run crossload adobe setup --from <activation directory or ZIP>"
                    .into(),
            ));
        }
        state.fulfilled.push(acsm.to_vec());
        state
            .books
            .iter()
            .find(|(a, _)| a == acsm)
            .map(|(_, book)| book.clone().map_err(AdeptError))
            .unwrap_or_else(|| Err(AdeptError("sim: no book for this ACSM".into())))
    }
}
