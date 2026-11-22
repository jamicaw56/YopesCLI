use super::*;
mod finish;
mod keys;
pub(crate) mod state;
mod text;
pub(crate) use self::state::*;

impl App {
    pub(crate) fn handle_connect_key(&mut self, key: KeyEvent) -> Action {
        let Some(stage) = self.connect.as_ref().map(|connect| connect.stage) else {
            return Action::None;
        };
        if self.connect.as_ref().is_some_and(|connect| connect.loading) {
            if key.code == KeyCode::Esc {
                self.connect = None;
            }
            return Action::None;
        }
        match stage {
            ConnectStage::Providers | ConnectStage::Models => self.handle_connect_list_key(key),
            ConnectStage::ApiKey => self.handle_connect_api_key(key),
            ConnectStage::AddProvider => self.handle_connect_add_provider(key),
            ConnectStage::RenameProvider => self.handle_connect_rename_provider(key),
            ConnectStage::Endpoint
            | ConnectStage::ModelsEndpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName => self.handle_connect_custom_text(key),
            ConnectStage::ModelContextWindow => self.handle_connect_context_key(key),
            ConnectStage::CustomModels => self.handle_connect_choice_key(key),
            ConnectStage::Password => self.handle_connect_password(key),
            ConnectStage::Unlock => self.handle_connect_unlock(key),
        }
    }

    /// Unlock a key that is already stored for the endpoint.
    ///
    /// Only the password is asked for: the key itself never has to be typed
    /// again.
    pub(crate) fn handle_connect_unlock(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    // Back to where the key was asked for.
                    connect.stage = if connect.custom_flow {
                        ConnectStage::CustomModels
                    } else {
                        ConnectStage::Models
                    };
                    connect.password.clear();
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            KeyCode::Enter => {
                let (password, endpoint) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (connect.password.clone(), connect.unlock_endpoint.clone())
                };
                let Some(secret) = self.stored_secret_for(&endpoint) else {
                    self.connect_error("That key is no longer stored.");
                    return Action::None;
                };
                let api_key = match secret.unlock(&password) {
                    Ok(api_key) => api_key,
                    Err(error) => {
                        self.connect_error(format!("Could not unlock the key: {error:#}"));
                        return Action::None;
                    }
                };
                // A custom endpoint keeps the unlocked key aside, so it
                // can be used without being written again.
                let custom_flow = self
                    .connect
                    .as_ref()
                    .is_some_and(|connect| connect.custom_flow);
                if custom_flow {
                    if let Some(connect) = self.connect.as_mut() {
                        connect.unlocked_key = Some(api_key);
                        connect.password.clear();
                    }
                    return self.finish_custom_connect(None);
                }
                return Action::None;
            }
            KeyCode::Backspace => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.password.pop();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(connect) = self.connect.as_mut() {
                    connect.password.push(character);
                }
            }
            _ => {}
        }
        Action::None
    }
}
