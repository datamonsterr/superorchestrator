//! The clap command tree (SPEC 15).

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

/// Command line interface for superorchestrator.
#[derive(Parser, Debug)]
#[command(
    name = "superorchestrator",
    version,
    about = "Delegate coding work across harnesses and accounts",
    after_help = "Get started: superorchestrator setup"
)]
pub struct Cli {
    /// Config directory (default: $SUPERORCHESTRATOR_CONFIG_DIR, else the XDG config dir).
    #[arg(long, global = true, value_name = "DIR")]
    pub config_dir: Option<PathBuf>,
    /// Print machine-readable JSON where a command lists things.
    #[arg(long, global = true)]
    pub json: bool,
    /// Do not ask for confirmation.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Guided first-run setup: config, accounts, sign-in and install
    Setup(SetupArgs),
    /// Create the config dir, seed config, levels and catalog
    Init,
    /// Show accounts, headroom and supervisor state
    Status,
    /// One snapshot for the monitor mod: supervisor, headroom and the session's runs
    Monitor(MonitorArgs),
    /// Manage accounts
    #[command(subcommand)]
    Account(AccountCmd),
    /// Route a task to a harness/model/effort, or evaluate the router
    Route(RouteArgs),
    /// Inspect and control runs
    #[command(subcommand)]
    Runs(RunsCmd),
    /// Attach to a run's native session
    Join(JoinArgs),
    /// Open the terminal UI: overview, accounts, runs and setup
    Tui,
    /// Start the web UI
    Ui(UiArgs),
    /// Run and inspect workflows
    #[command(subcommand)]
    Workflow(WorkflowCmd),
    /// Manage custom agents
    #[command(subcommand)]
    Agent(AgentCmd),
    /// Manage the model catalog
    #[command(subcommand)]
    Models(ModelsCmd),
    /// Read and write config.toml
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Check binaries, config, database, socket, UI port and sign-ins
    Doctor,
    /// Register the MCP server with harnesses and ship the skills
    Install(InstallArgs),
    /// Remove what `install` registered
    Uninstall(UninstallArgs),
    /// Move from the legacy setup
    Migrate(MigrateArgs),
    /// Manage run worktrees
    #[command(subcommand)]
    Worktrees(WorktreesCmd),
    /// Run the supervisor
    #[command(hide = true)]
    Daemon(DaemonArgs),
    /// Launch an account's harness (used by wrappers)
    #[command(hide = true)]
    Exec(ExecArgs),
    /// Run the MCP server on stdio
    #[command(hide = true)]
    Mcp(McpArgs),
}

#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Codex accounts to add (default: 1 when there is none yet)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..=99))]
    pub codex: Option<u32>,
    /// Claude accounts to add (default: 0; claude is kept for the orchestrator)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..=99))]
    pub claude: Option<u32>,
    /// OpenCode accounts to add (default: 1 when there is none yet)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..=99))]
    pub opencode: Option<u32>,
    /// Antigravity accounts to add (default: 1 when there is none yet)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..=99))]
    pub agy: Option<u32>,
    /// Do not sign in to the new accounts
    #[arg(long)]
    pub no_login: bool,
    /// Do not register the MCP server and skill
    #[arg(long)]
    pub no_install: bool,
    /// Do not add the delegation block to ~/.claude/CLAUDE.md
    #[arg(long)]
    pub no_claude_md: bool,
}

#[derive(Args, Debug)]
pub struct MonitorArgs {
    /// Session id; a unique prefix matches too
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Claude Code process id; matches the MCP bridge it opened
    #[arg(long, value_name = "PID")]
    pub client_pid: Option<i64>,
    /// Working directory of the session to match
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<String>,
    /// Minutes of history to draw
    #[arg(
        long,
        value_name = "MINUTES",
        default_value_t = 60,
        value_parser = clap::value_parser!(u32).range(5..=1440)
    )]
    pub window: u32,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Re-register the Claude Code entry even when it looks current.
    #[arg(long)]
    pub force: bool,
    /// Print what would change and write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Do not add the delegation block to ~/.claude/CLAUDE.md.
    #[arg(long)]
    pub no_claude_md: bool,
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Print what would change and write nothing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct McpArgs {
    /// Client name reported to the supervisor.
    #[arg(long, default_value = "mcp")]
    pub client: String,
}

#[derive(Subcommand, Debug)]
pub enum AccountCmd {
    /// Create a credential home, account file and wrapper
    Add(AccountAdd),
    /// Link an existing credential home, or scan for candidates
    Import(AccountImport),
    /// Sign in to an account
    Login { name: String },
    /// List accounts with sign-in state, load and headroom
    List,
    /// Remove an account
    Remove(AccountRemove),
    /// Enable an account
    Enable { name: String },
    /// Disable an account
    Disable { name: String },
    /// Put an account (pool) in manual cooldown, or clear it
    Flag(AccountFlag),
}

#[derive(Args, Debug)]
pub struct AccountAdd {
    /// codex, agy, opencode or claude
    pub harness: String,
    /// Account name (default: <harness>-<n> with the lowest free n)
    #[arg(long)]
    pub name: Option<String>,
    /// Create this many accounts, named <harness>-<n>
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=99))]
    pub count: u32,
    /// Credential home (default: ~/.<name>); needs --name
    #[arg(long, requires = "name")]
    pub home: Option<String>,
    /// Sign in to each new account in turn (needs a terminal; otherwise prints the commands)
    #[arg(long)]
    pub login: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("mode").required(true).args(["name", "scan"])))]
pub struct AccountImport {
    /// Account name
    pub name: Option<String>,
    #[arg(long, requires = "name")]
    pub harness: Option<String>,
    #[arg(long, requires = "name")]
    pub home: Option<String>,
    /// Detect existing homes and offer to import each
    #[arg(long)]
    pub scan: bool,
}

#[derive(Args, Debug)]
pub struct AccountRemove {
    pub name: String,
    /// Also delete the credential home
    #[arg(long)]
    pub purge: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("when").required(true).args(["until", "hours", "clear"])))]
pub struct AccountFlag {
    pub name: String,
    #[arg(long)]
    pub pool: Option<String>,
    /// Cooldown end as an RFC 3339 time
    #[arg(long)]
    pub until: Option<String>,
    /// Cooldown length in hours
    #[arg(long)]
    pub hours: Option<f64>,
    /// Remove the cooldown
    #[arg(long)]
    pub clear: bool,
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Scaffold a new agent in the config agents dir
    Create(AgentCreate),
    /// List agents from all search locations
    List,
    /// Show an agent's fields and per-harness gaps
    Show { name: String },
    /// Open an agent file in $EDITOR
    Edit { name: String },
    /// Delete an agent (not for `.claude/agents` imports)
    Remove { name: String },
}

#[derive(Args, Debug)]
pub struct AgentCreate {
    pub name: String,
    /// auto, codex, agy, opencode or claude
    #[arg(long)]
    pub harness: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ModelsCmd {
    /// Collect models from the harness CLIs and models.dev into models.toml
    Refresh,
    /// List the catalog
    List,
}

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RouteArgs {
    /// Task text to route. Several words need no quotes; put `--` before a task that
    /// starts with `eval` or `-`
    #[arg(num_args = 1.., value_name = "TASK")]
    pub task: Vec<String>,
    /// Pin the route: harness/model[/effort], or a model name
    #[arg(long, value_name = "REF")]
    pub model: Option<String>,
    /// Agent profile whose instructions the decider sees
    #[arg(long, value_name = "NAME")]
    pub agent: Option<String>,
    #[arg(long)]
    pub explain: bool,
    #[command(subcommand)]
    pub sub: Option<RouteSub>,
}

#[derive(Subcommand, Debug)]
pub enum RouteSub {
    /// Evaluate the router against labelled cases
    Eval {
        #[arg(long)]
        cases: Option<PathBuf>,
        #[arg(long)]
        require_live: bool,
        #[arg(long)]
        min_accuracy: Option<f64>,
        /// Fail when the share of cases scored inside `expected_complexity` is lower (0 to 1)
        #[arg(long)]
        min_complexity_accuracy: Option<f64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RunsCmd {
    /// List active runs (all runs with --all)
    List {
        /// Include finished runs
        #[arg(long)]
        all: bool,
    },
    /// Show one run: state, recent events, result, usage and join command
    Show {
        run: String,
        /// Also print what the run changed in its worktree (stat and patch)
        #[arg(long)]
        diff: bool,
    },
    /// Follow a run's events (or a workflow's steps) until it finishes
    Watch(WatchArgs),
    /// Cancel a run and its subtree
    Cancel { run: String },
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("target").required(true).args(["run", "workflow"])))]
pub struct WatchArgs {
    /// Run id or unique prefix
    pub run: Option<String>,
    /// Follow a workflow run's steps instead
    #[arg(long, value_name = "ID")]
    pub workflow: Option<String>,
}

#[derive(Args, Debug)]
pub struct JoinArgs {
    pub run: String,
    /// Run the join command instead of printing it
    #[arg(long)]
    pub exec: bool,
}

#[derive(Subcommand, Debug)]
pub enum WorktreesCmd {
    /// List run worktrees
    List,
    /// Remove the worktrees and branches of finished runs
    Prune,
}

#[derive(Subcommand, Debug)]
pub enum WorkflowCmd {
    /// Run a saved workflow, or a script file, and print its result
    Run(WorkflowRun),
    /// List saved workflows and recent workflow runs
    List,
    /// Show a workflow run: state, steps and result
    Status {
        /// Workflow run id or unique prefix
        id: String,
    },
    /// Stop a running workflow and its steps
    Cancel {
        /// Workflow run id or unique prefix
        id: String,
    },
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("what").required(true).args(["name", "file"])))]
pub struct WorkflowRun {
    /// Saved workflow name (`<config>/workflows/<name>.js` or `.superorchestrator/workflows`)
    pub name: Option<String>,
    /// JavaScript file to run instead of a saved workflow
    #[arg(long, value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// JSON passed to the script as `args`
    #[arg(long, value_name = "JSON")]
    pub args: Option<String>,
    /// Print the run id and return without waiting for the result
    #[arg(long)]
    pub detach: bool,
    /// Run every step again instead of replaying cached results
    #[arg(long)]
    pub fresh: bool,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print a value (exit 1 when unset)
    Get { key: String },
    /// Set a value, keeping comments
    Set { key: String, value: String },
    /// Open config.toml in $EDITOR and validate it afterwards
    Edit,
}

#[derive(Args, Debug)]
pub struct UiArgs {
    #[arg(long)]
    pub bind: Option<String>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub token: Option<String>,
    /// Write a systemd user unit or launchd agent
    #[arg(long)]
    pub install_service: bool,
    /// Do not open the browser
    #[arg(long)]
    pub no_open: bool,
}

#[derive(Args, Debug)]
pub struct DaemonArgs {
    /// Log to stderr instead of logs/supervisor.log
    #[arg(long)]
    pub foreground: bool,
}

#[derive(Args, Debug)]
pub struct ExecArgs {
    pub account: String,
    /// Arguments for the harness binary (after `--`)
    #[arg(last = true)]
    pub args: Vec<String>,
}

#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// Apply the migration instead of only reporting it
    #[arg(long)]
    pub apply: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(args: &[&str]) -> RouteArgs {
        let mut argv = vec!["superorchestrator", "route"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv).unwrap().command {
            Some(Command::Route(a)) => a,
            other => panic!("not a route command: {other:?}"),
        }
    }

    #[test]
    fn route_joins_unquoted_words_and_keeps_flags_after_them() {
        let a = route(&["fix", "a", "typo", "--explain"]);
        assert_eq!(a.task.join(" "), "fix a typo");
        assert!(a.explain);
        assert!(a.sub.is_none());
    }

    #[test]
    fn route_double_dash_lets_a_task_start_with_eval() {
        let a = route(&["--", "eval", "foo"]);
        assert_eq!(a.task.join(" "), "eval foo");
        assert!(a.sub.is_none());
        assert!(matches!(route(&["eval"]).sub, Some(RouteSub::Eval { .. })));
    }
}
