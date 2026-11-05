//! `dry-run`: show what would be backed up / forgotten, change nothing.

use super::{Ctx, run};
use crate::error::Result;

pub fn execute(ctx: &Ctx) -> Result<()> {
    run::execute(ctx, true)
}
