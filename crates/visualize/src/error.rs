use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no microphone is available")]
    NoInputDevice,
    #[error("no microphone matches {0:?}")]
    DeviceNotFound(String),
    #[error("microphone access was denied; allow it in the system's privacy settings")]
    PermissionDenied,
    #[error("the microphone offers no sample format this listener can read")]
    NoSampleFormat,
    #[error("microphone: {0}")]
    Audio(#[from] cpal::Error),
    #[error("the microphone thread stopped before it was ready")]
    ThreadStopped,
}
