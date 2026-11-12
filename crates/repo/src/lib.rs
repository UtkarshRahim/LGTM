pub mod checkout;
pub mod diff;
pub mod git;
pub mod mirror;
pub mod project;
pub mod remote;
pub mod watch;

pub use diff::{DiffLine, FileDiff, FileStatus, Hunk, LineKind};
pub use git::{Git, GitError, GitResult};
pub use mirror::{MirrorRepo, Mirrors};
pub use project::{ProjectInfo, ProjectStatus};
pub use remote::RemoteRepo;
