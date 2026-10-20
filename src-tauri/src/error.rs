use serde::{Serialize, Serializer};

/// Commands surface failures to the UI as plain messages; the UI only logs or shows them.
#[derive(Debug)]
pub struct CommandError(String);

pub type CommandResult<T> = Result<T, CommandError>;

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for CommandError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl From<antistreaming_library::Error> for CommandError {
    fn from(error: antistreaming_library::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<antistreaming_visualize::Error> for CommandError {
    fn from(error: antistreaming_visualize::Error) -> Self {
        Self(error.to_string())
    }
}
