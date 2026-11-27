mod account_manager;
mod action;
mod app_server;
mod discovery;
mod render;
mod settings;
mod usage;

use openaction::{OpenActionResult, register_action, run};
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use crate::account_manager::AccountManager;
use crate::action::CodexLimitsAction;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    if let Err(error) = TermLogger::init(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Stderr,
        ColorChoice::Never,
    ) {
        eprintln!("Logger initialization failed: {error}");
    }

    let manager = AccountManager::start();
    register_action(CodexLimitsAction::new(manager)).await;

    run(std::env::args().collect()).await
}
