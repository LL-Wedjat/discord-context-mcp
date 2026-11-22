use anyhow::{Context, Result};
use keyring::{Entry, Error as KeyringError};

const SERVICE_NAME: &str = "discord-context-mcp";
const USER_NAME: &str = "default";

pub fn store_token(token: &str) -> Result<()> {
    let entry = Entry::new(SERVICE_NAME, USER_NAME).context("Failed to create keyring entry")?;

    // set_password overwrites any existing credential in place; do not pre-delete (avoids losing a working token on failure).
    entry
        .set_password(token)
        .context("Failed to store token in Windows Credential Manager")?;
    Ok(())
}

pub fn load_token() -> Result<Option<String>> {
    let entry = Entry::new(SERVICE_NAME, USER_NAME).context("Failed to create keyring entry")?;

    match entry.get_password() {
        Ok(pass) => Ok(Some(pass)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(e) => {
            // Check string error representations in case of platform-specific NoEntry mapping
            let err_str = e.to_string();
            if err_str.contains("not found")
                || err_str.contains("Element not found")
                || err_str.contains("NoEntry")
            {
                Ok(None)
            } else {
                Err(e).context("Failed to read token from Windows Credential Manager")
            }
        }
    }
}

pub fn delete_token() -> Result<()> {
    let entry = Entry::new(SERVICE_NAME, USER_NAME).context("Failed to create keyring entry")?;

    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("not found")
                || err_str.contains("Element not found")
                || err_str.contains("NoEntry")
            {
                Ok(())
            } else {
                Err(e).context("Failed to delete token from Windows Credential Manager")
            }
        }
    }
}
