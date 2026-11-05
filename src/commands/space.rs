//! `space`: show free space and repository size of the repository target.
//! Reads only the target, so it needs neither restic nor the password.

use super::Ctx;
use crate::error::{AppError, Result};
use crate::space::{self, SpaceError};

pub fn execute(ctx: &Ctx) -> Result<()> {
    if space::repository(&ctx.cfg).is_none() {
        return Err(AppError::config(
            "repository.url (or repository.url_file) is not set",
        ));
    }
    match space::target_free_space(&ctx.cfg) {
        Ok(s) => {
            println!("{s}");
            Ok(())
        }
        Err(SpaceError::Unsupported) => Err(AppError::fail(
            "free space is only available for local and sftp: repositories",
        )),
        Err(SpaceError::Failed) => Err(AppError::fail("cannot read free space of the repository target")),
    }
}
