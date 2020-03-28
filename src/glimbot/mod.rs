use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error as StdError;
use std::path::{PathBuf};
use std::result::Result as StdResult;
use std::sync::Arc;

use diesel::{ExpressionMethods, insert_or_ignore_into, RunQueryDsl};
use log::{debug, error, info, trace};
use parking_lot::{Mutex, RwLock};
use serenity::http::CacheHttp;
use serenity::model::event::{Event, EventType};
use serenity::model::gateway::Ready;
use serenity::model::id::{ChannelId, MessageId, UserId};
use serenity::model::prelude::{GuildId, Message};
use serenity::prelude::{Context, EventHandler as EHandler};
use serenity::utils::MessageBuilder;
use thiserror::Error;

use crate::diesel::QueryDsl;
use crate::glimbot::db::{Conn, DBPool, pooled_connection};
use crate::glimbot::modules::Module;
use crate::glimbot::modules::command::{Commander, CommanderError};
use crate::glimbot::modules::command::parser::RawCmd;
use crate::glimbot::util::FromError;
use std::sync::atomic::AtomicBool;

pub mod env;
pub mod config;
pub mod modules;
pub mod util;
pub mod db;
pub(crate) mod schema;


pub type EventHandlerFn = fn(&GlimDispatch, GuildId, &Context, &Event) -> bool;
pub type MessageHandlerFn = fn(&GlimDispatch, GuildId, &Context, &Message) -> bool;
pub type CommandHandlerFn = fn(&GlimDispatch, GuildId, &Context, &Message, RawCmd) -> modules::command::Result<RawCmd>;

#[derive(Clone)]
pub enum EventHandler {
    GenericHandler(EventHandlerFn),
    MessageHandler(MessageHandlerFn),
    CommandHandler(CommandHandlerFn),
}

impl From<CommandHandlerFn> for EventHandler {
    fn from(f: CommandHandlerFn) -> Self {
        EventHandler::CommandHandler(f)
    }
}

impl From<MessageHandlerFn> for EventHandler {
    fn from(f: MessageHandlerFn) -> Self {
        EventHandler::MessageHandler(f)
    }
}

#[derive(Error, Debug)]
pub enum EventError {
    #[error("An error occurred: {0}")]
    Other(#[from] Box<dyn StdError>),
}

#[derive(Error, Debug)]
pub enum InternalError {
    #[error("An error occurred: {0:?}")]
    Other(#[from] Box<dyn StdError>),
}

impl FromError for InternalError {
    fn from_error(e: impl StdError + 'static) -> Self {
        InternalError::Other(Box::from(e))
    }
}

pub type EventResult<T> = StdResult<T, EventError>;
pub type GlimResult<T> = StdResult<T, InternalError>;

pub struct GlimDispatch {
    working_directory: PathBuf,
    modules: HashMap<String, Module>,
    hooks: BTreeMap<EventType, Vec<EventHandler>>,
    command_map: HashMap<String, String>,
    db_conn: DBPool,
    wr_conn: Arc<Mutex<Conn>>,
    owners: Arc<RwLock<HashSet<UserId>>>,
    shutdown: AtomicBool,
}

impl GlimDispatch {
    pub fn new() -> Self {
        let pool = pooled_connection("./glimbot.db");
        let c = pool.get().unwrap();

        GlimDispatch {
            working_directory: PathBuf::from(".".to_string()),
            modules: HashMap::new(),
            hooks: BTreeMap::new(),
            db_conn: pool,
            wr_conn: Arc::new(Mutex::new(c)),
            command_map: HashMap::new(),
            owners: Arc::new(RwLock::new(HashSet::new())),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn with_module(mut self, module: Module) -> Self {
        module.hooks().iter()
            .for_each(|(ev, handler)| {
                let entry = self.hooks.entry(ev.clone()).or_default();
                entry.push(handler.clone())
            });
        module.commands().keys().for_each(|x| { self.command_map.insert(x.clone(), module.name().to_string()); });
        info!("Loaded module {}", module.name());
        self.modules.insert(module.name().to_string(), module);
        self
    }

    // Checks to see if we've met this guild before; if not it creates a default config for it.
    // In either case, returns a guild context associated with this id
    pub fn encounter_guild(&self, g: GuildId) {
        use schema::guilds::dsl::*;
        if guilds.count().filter(id.eq(g.0 as i64)).get_result::<i64>(&self.rd_conn()).unwrap() == 0 {
            let new = insert_or_ignore_into(guilds)
                .values(id.eq(g.0 as i64))
                .execute(self.wr_conn().lock().as_ref()).unwrap();

            if new > 0 {
                info!("Encountered new guild {}", g)
            }
        }
    }

    pub fn rd_conn(&self) -> Conn {
        self.db_conn.get().expect("Couldn't connect to database!")
    }
    pub fn wr_conn(&self) -> &Mutex<Conn> { self.wr_conn.as_ref() }

    pub fn ensure_module_config(&self, g: GuildId, module: impl AsRef<str>) {
        let module = module.as_ref();
        let mod_info = self.modules.get(module).unwrap();
        self.encounter_guild(g);
        mod_info.write_default_config(self, g)
    }

    pub fn resolve_command(&self, s: impl AsRef<str>) -> Option<&Commander> {
        let s = s.as_ref();
        self.command_map.get(s)
            .and_then(|s| self.modules.get(s))
            .and_then(|m| m.commands().get(s))
    }
}

impl EHandler for GlimDispatch {
    fn message(&self, ctx: Context, new_message: Message) {
        use schema::guilds::dsl::*;

        if new_message.is_own(&ctx) {
            trace!("Saw a message from myself.");
            return;
        } else {
            trace!("Saw a message from {}", new_message.author.id);
        };
        if let Some(gid) = new_message.guild_id {
            self.encounter_guild(gid);
            if let Some(v) = self.hooks.get(&EventType::MessageCreate) {
                let mut stop = false;
                for hook in v {
                    match hook {
                        EventHandler::MessageHandler(m) => {
                            stop = m(self, gid, &ctx, &new_message);
                        }
                        EventHandler::CommandHandler(_) => (),
                        _ => unreachable!()
                    };

                    if stop {
                        return;
                    }
                };
            }

            let pref: String = guilds.select(command_prefix).filter(id.eq(gid.0 as i64)).first(&self.rd_conn()).unwrap();
            if new_message.content.starts_with(&pref) {
                // This may be a command
                let raw_cmd = modules::command::parser::parse_command(&new_message.content);

                let cmd: modules::command::Result<RawCmd> = if let Some(v) = self.hooks.get(&EventType::MessageCreate) {
                    raw_cmd.and_then(|rc| {
                        v.iter()
                            .filter(|e| match e {
                                EventHandler::CommandHandler(_) => { true }
                                _ => { false }
                            })
                            .try_fold(rc, |s, h| {
                                if let EventHandler::CommandHandler(c) = h {
                                    c(self, gid, &ctx, &new_message, s)
                                } else {
                                    unreachable!()
                                }
                            })
                    })
                } else {
                    raw_cmd
                };


                match cmd.and_then(|r| {
                    let module = self.command_map.get(&r.command);
                    if let Some(_name) = module {
                        let c = self.resolve_command(&r.command).unwrap();
                        c.invoke(self, gid, &ctx, &new_message, &r.args)
                    } else {
                        debug!("Got invalid command in channel {}: {}", new_message.channel_id, &r.command);
                        new_message.channel_id.say(&ctx, "```No such command.```")
                            .map(|_x| {})
                            .map_err(|_| CommanderError::Silent)
                    }
                }) {
                    Err(CommanderError::Silent) => {}
                    Err(CommanderError::SilentError(e)) => {
                        debug!("Command failed: {}", e);
                    }
                    Err(e) => {
                        if let Err(err) = new_message.channel_id.say(&ctx, MessageBuilder::new()
                            .push_codeblock_safe(e, None)
                            .build()) {
                            debug!("Command failed: {}", err);
                        }
                    }
                    Ok(_) => {}
                }
            }
        }
    }

    fn message_delete(&self, _ctx: Context, _channel_id: ChannelId, _deleted_message_id: MessageId) {}

    fn message_delete_bulk(&self, _ctx: Context, _channel_id: ChannelId, _multiple_deleted_messages_ids: Vec<MessageId>) {}

    fn ready(&self, ctx: Context, data_about_bot: Ready) {
        use serenity::model::gateway::Activity;
        info!("Connected to Discord!");

        data_about_bot.guilds.iter().for_each(
            |g| {
                self.encounter_guild(g.id());
            }
        );

        let owners = match ctx.http().get_current_application_info() {
            Ok(info) => {
                let mut owners = HashSet::new();
                owners.insert(info.owner.id);
                owners
            }
            Err(why) => {
                error!("Could not access application info: {:?}", why);
                HashSet::new()
            }
        };

        {
            let mut dest = self.owners.write();
            owners.into_iter().for_each(|id| { dest.insert(id); });
        };

        self.owners.read().iter().for_each(
            |id| debug!("Found owner {}", ctx.http()
                .get_user(id.0)
                .map(|u| u.name)
                .unwrap_or_else(|_| id.to_string()))
        );

        ctx.set_activity(Activity::playing("Cultist Simulator"));
    }
}