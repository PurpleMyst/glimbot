
use std::collections::{BTreeMap, HashMap};
use serenity::model::id::GuildId;
use serenity::model::permissions::Permissions;
use serenity::model::prelude::EventType;

use crate::glimbot::{EventHandler, GlimDispatch};
use crate::glimbot::modules::command::Commander;

pub mod command;
pub mod ping;
pub mod help;
pub mod bag;
pub mod incrementer;
pub mod bot_admin;
pub mod dice;

pub type ConfigFn = fn(&GlimDispatch, GuildId) -> ();

#[derive(Clone)]
pub struct Module {
    name: String,
    commands: HashMap<String, Commander>,
    hooks: BTreeMap<EventType, EventHandler>,
    create_config: ConfigFn,
    req_permissions: Permissions,
}

impl Module {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn commands(&self) -> &HashMap<String, Commander> {
        &self.commands
    }

    pub fn hooks(&self) -> &BTreeMap<EventType, EventHandler> {
        &self.hooks
    }

    pub fn write_default_config(&self, disp: &GlimDispatch, g: GuildId) {
        (self.create_config)(disp, g)
    }

    pub fn required_perms(&self) -> Permissions {
        self.req_permissions
    }
}

#[derive(Clone)]
pub struct ModuleBuilder {
    module: Module
}

pub fn default_config(_: &GlimDispatch, _: GuildId) {}

impl ModuleBuilder {
    pub fn new(name: impl Into<String>) -> ModuleBuilder {
        ModuleBuilder {
            module: Module {
                name: name.into(),
                commands: HashMap::new(),
                hooks: BTreeMap::new(),
                create_config: default_config,
                req_permissions: Permissions::default(),
            }
        }
    }

    pub fn with_command(mut self, cmd: Commander) -> Self {
        self.module.req_permissions |= cmd.permissions();
        self.module.commands.insert(cmd.name().to_string(), cmd);
        self
    }

    pub fn with_default_config_gen(mut self, conf: ConfigFn) -> Self {
        self.module.create_config = conf;
        self
    }

    pub fn with_hook(mut self, typ: EventType, hook: EventHandler) -> Self {
        match &typ {
            EventType::MessageCreate => {
                match &hook {
                    EventHandler::MessageHandler(_) => {}
                    EventHandler::CommandHandler(_) => {}
                    _ => panic!("MessageCreate cannot use GenericHandlers")
                }
            }
            EventType::MessageDelete |
            EventType::MessageDeleteBulk |
            EventType::MessageUpdate => {
                match &hook {
                    EventHandler::MessageHandler(_) => {}
                    _ => panic!("MessageUpdate/Delete/DeleteBulk can only use MessageHandler")
                }
            }
            _ => {
                match &hook {
                    EventHandler::GenericHandler(_) => {}
                    _ => panic!("All non-message events must use GenericHandler")
                }
            }
        };

        self.module.hooks.insert(typ, hook);
        self
    }

    pub fn build(self) -> Module {
        self.module
    }
}