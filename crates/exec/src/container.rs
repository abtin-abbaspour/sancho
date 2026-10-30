//! Runs commands inside a podman container: root commands as root, others as
//! an unprivileged user. Used by integration tests only.

use std::io;

use crate::{run_argv, Executor, Invocation, Output};

#[derive(Debug, Clone)]
pub struct ContainerExecutor {
    pub container: String,
    /// User for non-root commands.
    pub user: String,
}

impl Executor for ContainerExecutor {
    fn run(&mut self, inv: &Invocation) -> io::Result<Output> {
        let user = if inv.root { "root" } else { self.user.as_str() };
        let mut argv = vec!["podman", "exec", "--user", user];
        if inv.stdin.is_some() {
            argv.push("-i");
        }
        argv.extend(["--env", "LC_ALL=C", self.container.as_str()]);
        argv.extend(inv.argv.iter().map(String::as_str));
        // Never attach the container to the user's terminal in tests.
        let inv = Invocation {
            interactive: false,
            ..inv.clone()
        };
        run_argv(&argv, &inv)
    }
}
