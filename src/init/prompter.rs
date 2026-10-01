//! The interactive prompts the init wizard performs, behind one trait so the
//! wizard can be driven without a terminal. `InquirePrompter` is the only
//! production code in the crate that calls `inquire`; it reproduces the exact
//! texts, defaults, help messages and skip semantics the wizard used inline.

use anyhow::Result;
use inquire::{Confirm, MultiSelect, Select, Text};

pub(crate) trait Prompter {
    /// Single choice. `None` = skipped (esc / Ctrl-C).
    fn select(
        &mut self,
        prompt: &str,
        options: Vec<String>,
        help: Option<&str>,
    ) -> Result<Option<String>>;

    /// Multi choice. A skipped prompt yields an empty selection.
    fn multi_select(
        &mut self,
        prompt: &str,
        options: Vec<String>,
        defaults: Vec<usize>,
        help: Option<&str>,
        page_size: Option<usize>,
    ) -> Result<Vec<String>>;

    /// Yes/no.
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool>;

    /// Free text. `default: Some(_)` reproduces the wizard's non-skippable
    /// `.with_default(..).prompt()` call (used only for the optional build
    /// description); `None` reproduces `.prompt_skippable()` and so maps
    /// esc / Ctrl-C to `None`.
    fn text(&mut self, prompt: &str, default: Option<&str>) -> Result<Option<String>>;
}

pub(crate) struct InquirePrompter;

impl Prompter for InquirePrompter {
    fn select(
        &mut self,
        prompt: &str,
        options: Vec<String>,
        help: Option<&str>,
    ) -> Result<Option<String>> {
        let mut select = Select::new(prompt, options);
        if let Some(help) = help {
            select = select.with_help_message(help);
        }
        Ok(select.prompt_skippable()?)
    }

    fn multi_select(
        &mut self,
        prompt: &str,
        options: Vec<String>,
        defaults: Vec<usize>,
        help: Option<&str>,
        page_size: Option<usize>,
    ) -> Result<Vec<String>> {
        let mut select = MultiSelect::new(prompt, options).with_default(&defaults);
        if let Some(help) = help {
            select = select.with_help_message(help);
        }
        if let Some(size) = page_size {
            select = select.with_page_size(size);
        }
        Ok(select.prompt_skippable()?.unwrap_or_default())
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool> {
        Ok(Confirm::new(prompt).with_default(default).prompt()?)
    }

    fn text(&mut self, prompt: &str, default: Option<&str>) -> Result<Option<String>> {
        let text = Text::new(prompt);
        match default {
            Some(value) => Ok(Some(text.with_default(value).prompt()?)),
            None => Ok(text.prompt_skippable()?),
        }
    }
}

#[cfg(test)]
pub(crate) struct ScriptedPrompter {
    selects: std::collections::VecDeque<Option<String>>,
    multis: std::collections::VecDeque<Vec<String>>,
    confirms: std::collections::VecDeque<bool>,
    texts: std::collections::VecDeque<Option<String>>,
    /// Every prompt text, in call order, for the "same prompts/order" guard.
    pub prompted: Vec<String>,
    /// Every multi-select's option list, for the "same options" guard.
    pub multis_options: Vec<Vec<String>>,
}

#[cfg(test)]
impl ScriptedPrompter {
    pub fn new() -> Self {
        Self {
            selects: std::collections::VecDeque::new(),
            multis: std::collections::VecDeque::new(),
            confirms: std::collections::VecDeque::new(),
            texts: std::collections::VecDeque::new(),
            prompted: Vec::new(),
            multis_options: Vec::new(),
        }
    }

    pub fn with_selects(mut self, answers: Vec<Option<String>>) -> Self {
        self.selects = answers.into();
        self
    }

    pub fn with_multis(mut self, answers: Vec<Vec<String>>) -> Self {
        self.multis = answers.into();
        self
    }

    pub fn with_confirms(mut self, answers: Vec<bool>) -> Self {
        self.confirms = answers.into();
        self
    }

    pub fn with_texts(mut self, answers: Vec<Option<String>>) -> Self {
        self.texts = answers.into();
        self
    }
}

#[cfg(test)]
impl Prompter for ScriptedPrompter {
    fn select(
        &mut self,
        prompt: &str,
        _options: Vec<String>,
        _help: Option<&str>,
    ) -> Result<Option<String>> {
        self.prompted.push(prompt.to_string());
        self.selects.pop_front().map_or_else(
            || anyhow::bail!("ScriptedPrompter: no scripted select for {prompt:?}"),
            Ok,
        )
    }

    fn multi_select(
        &mut self,
        prompt: &str,
        options: Vec<String>,
        _defaults: Vec<usize>,
        _help: Option<&str>,
        _page_size: Option<usize>,
    ) -> Result<Vec<String>> {
        self.prompted.push(prompt.to_string());
        self.multis_options.push(options);
        self.multis.pop_front().map_or_else(
            || anyhow::bail!("ScriptedPrompter: no scripted multi_select for {prompt:?}"),
            Ok,
        )
    }

    fn confirm(&mut self, prompt: &str, _default: bool) -> Result<bool> {
        self.prompted.push(prompt.to_string());
        self.confirms.pop_front().map_or_else(
            || anyhow::bail!("ScriptedPrompter: no scripted confirm for {prompt:?}"),
            Ok,
        )
    }

    fn text(&mut self, prompt: &str, _default: Option<&str>) -> Result<Option<String>> {
        self.prompted.push(prompt.to_string());
        self.texts.pop_front().map_or_else(
            || anyhow::bail!("ScriptedPrompter: no scripted text for {prompt:?}"),
            Ok,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_prompt_builders_construct() {
        // The exact builder chains run() and its helpers use; a dependency
        // bump that changes this API must fail here, not at prompt time.
        let _saved_builds = Select::new(
            "Saved builds available",
            vec!["Start fresh configuration".to_string()],
        )
        .with_help_message("↑↓ move  enter confirm  esc start fresh");
        let _hooks = MultiSelect::new("Hooks", vec!["item".to_string()])
            .with_default(&[0usize])
            .with_help_message("↑↓ move  space select  enter confirm  esc skip");
        let _custom = Select::new("Hook type", crate::hooks::valid_hook_names().to_vec());
        let _command = Text::new("  Command to run");
        let _templates = MultiSelect::new(".gitignore templates", vec!["rust".to_string()])
            .with_help_message("Type to filter  ↑↓ move  space select  enter confirm  esc skip")
            .with_page_size(10);
        let _attrs = MultiSelect::new(".gitattributes", vec!["line-endings".to_string()])
            .with_default(&[0usize])
            .with_help_message("space select  enter confirm  esc skip");
        let _git_config = MultiSelect::new("Git config", vec!["label".to_string()])
            .with_default(&[0usize])
            .with_help_message("↑↓ move  space select  enter confirm  esc skip");
        let _apply = Confirm::new("Apply these changes?").with_default(true);
        let _save_build =
            Confirm::new("Save this configuration as a reusable build?").with_default(false);
        let _name = Text::new("  Build name");
        let _overwrite = Select::new(
            "  A build with that name already exists",
            vec![
                "Choose a different name".to_string(),
                "Overwrite existing build 'x'".to_string(),
            ],
        );
    }
}
