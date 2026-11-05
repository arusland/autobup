//! `notify-test`: send a test message to the configured Telegram chat.

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{log, telegram};

pub fn execute(ctx: &Ctx) -> Result<()> {
    if !ctx.cfg.telegram_configured() {
        return Err(AppError::config(
            "set telegram.bot_token and telegram.chat_id first",
        ));
    }
    let msg = format!(
        "🔔 [{}] autobup test notification from {}",
        ctx.cfg.project_name,
        telegram::hostname()
    );
    telegram::send(&ctx.cfg, &msg)
        .map_err(|e| AppError::fail(format!("could not send Telegram message: {e}")))?;
    log::info("Telegram test message sent");
    Ok(())
}
