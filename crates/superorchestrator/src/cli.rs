//! The clap command tree (SPEC 15).

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

/// Command line interface for superorchestrator.
#[derive(Parser, Debug)]
#[command(
    name = "superorchestrator",
    version,
    about = "Delegate coding work across harnesses and accounts"
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
    /// Create the config dir, seed config, choices and catalog
    Init,
    /// Register the MCP server with Claude Code and other harnesses
    Install(InstallArgs),
    /// Remove what `install` registered
    Uninstall(UninstallArgs),
    /// Check binaries, config, database, socket, UI port and sign-ins
    Doctor,
    /// Show accounts, headroom and supervisor state
    Status,
    /// Manage accounts
    #[command(subcommand)]
    Account(AccountCmd),
    /// Manage custom agents
    #[command(subcommand)]
    Agent(AgentCmd),
    /// Manage the model catalog
    #[command(subcommand)]
    Models(ModelsCmd),
    /// Route a task to a choice, or evaluate the router
    Route(RouteArgs),
    /// Inspect and control runs
    #[command(subcommand)]
    Runs(RunsCmd),
    /// Attach to a run's native session
    Join(JoinArgs),
    /// Manage run worktrees
    #[command(subcommand)]
    Worktrees(WorktreesCmd),
    /// Run and inspect workflows
    #[command(subcommand)]
    Workflow(WorkflowCmd),
    /// Read and write config.toml
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Start the web UI
    Ui(UiArgs),
    /// Run the supervisor
    Daemon(DaemonArgs),
    /// Launch an account's harness (used by wrappers)
    Exec(ExecArgs),
    /// Run the MCP server on stdio
    Mcp(McpArgs),
    /// Move from the legacy setup
    Migrate(MigrateArgs),
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Re-register the Claude Code entry even when it looks current.
    #[arg(long)]
    pub force: bool,
    /// Print what would change and write nothing.
    #[arg(long)]
    pub dry_run: bool,
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
    #[arg(long)]
    pub name: String,
    /// Credential home (default: ~/.<name>)
    #[arg(long)]
    pub home: Option<String>,
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
    Create,
    List,
    Show { name: String },
    Edit { name: String },
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ModelsCmd {
    Refresh,
    List,
}

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RouteArgs {
    /// Task text to route
    pub task: Option<String>,
    #[arg(long)]
    pub role: Option<String>,
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
    Show { run: String },
    /// Follow a run's events until it finishes
    Watch { run: String },
    /// Cancel a run and its subtree
    Cancel { run: String },
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
    Run { name: String },
    List,
    Status { id: String },
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
