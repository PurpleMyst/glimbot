//  Glimbot - A Discord anti-spam and administration bot.
//  Copyright (C) 2020 Nick Samson

//  This program is free software: you can redistribute it and/or modify
//  it under the terms of the GNU General Public License as published by
//  the Free Software Foundation, either version 3 of the License, or
//  (at your option) any later version.

//  This program is distributed in the hope that it will be useful,
//  but WITHOUT ANY WARRANTY; without even the implied warranty of
//  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//  GNU General Public License for more details.

//  You should have received a copy of the GNU General Public License
//  along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Module for managing roles and ensuring users can't run restricted (admin-only) commands.

use crate::modules::Module;
use crate::modules::config;
use crate::dispatch::Dispatch;
use serenity::prelude::Context;
use serenity::model::prelude::Message;
use std::borrow::Cow;
use serenity::model::id::{UserId, RoleId};
use crate::db::cache::get_cached_connection;
use crate::modules::hook::Error::{DeniedWithReason, NeedRole};
use once_cell::unsync::Lazy;
use clap::{App, Arg, AppSettings, SubCommand, ArgMatches};
use crate::error::AnyError;
use std::str::{FromStr, ParseBoolError};
use crate::modules::commands::Command;
use crate::args::parse_app_matches;
use crate::modules::config::fallible_validator;
use serenity::utils::MessageBuilder;

static ADMIN_KEY: &'static str = "admin_role";

fn role_hook<'a, 'b, 'c, 'd>(disp: &'a Dispatch, ctx: &'b Context, msg: &'c Message, name: Cow<'d, str>) -> super::hook::Result<Cow<'d, str>> {
    trace!("Applying role hook.");
    let guild = msg.guild_id.unwrap();

    let owner: UserId = ctx.cache.read().guild(guild).unwrap().read().owner_id;
    let author = msg.author.id;
    if owner == author {
        trace!("User is server owner.");
        return Ok(name);
    }

    let conn = get_cached_connection(guild)?;
    let rconn = conn.as_ref().borrow();

    let admin_role: RoleId = disp.get_config(&rconn, ADMIN_KEY)?.parse::<u64>().unwrap().into();
    if msg.author.has_role(ctx, guild, admin_role).map_err(AnyError::boxed)? {
        trace!("User is admin.");
        return Ok(name);
    }

    // Now we need to see if the desired command is sensitive or not.
    let module = disp.modules().get(name.as_ref()).ok_or(DeniedWithReason("No such command.".into()))?;
    if module.sensitive || {
        rconn.as_ref().query_row(
            "SELECT ? IN restricted_commands;",
            params![name.as_ref()],
            |r| r.get(0),
        ).map_err(crate::db::DatabaseError::SQLError)?
    } {
        trace!("Command is sensitive and user is not admin or owner.");
        let full_guild = ctx.cache.read().guild(guild).unwrap();
        let role_name = full_guild.read().roles.get(&admin_role).ok_or(DeniedWithReason("Not an admin or admin role outdated.".into()))?.name.clone();

        let needed_role = vec![role_name];
        Err(NeedRole(needed_role))
    } else {
        trace!("Command not sensitive.");
        Ok(name)
    }
}

thread_local! {
static PARSER: Lazy<App<'static, 'static>> = Lazy::new(
    || {
        let role_id = Arg::with_name("role-id")
            .value_name("ROLE")
            .help("Any string interpretable as a Discord role snowflake.")
            .takes_value(true)
            .required(true);

        let user_id = Arg::with_name("user-id")
            .value_name("USER")
            .help("Any string interpretable as a Discord user snowflake.")
            .takes_value(true)
            .required(true);

        App::new("roles")
            .arg(role_id.clone())
            .about("Command for administering user roles. Non-admins probably want the \"me\" command")
            .setting(AppSettings::SubcommandRequiredElseHelp)
            .subcommand(SubCommand::with_name("add-user")
                .arg(user_id.clone())
                .about("Adds a role to a user."))
            .subcommand(SubCommand::with_name("rem-user")
                .arg(user_id.clone())
                .about("Removes a role from a user.")
            )
            .subcommand(
            SubCommand::with_name("set-joinable")
                .arg(Arg::with_name("joinable")
                    .validator(fallible_validator::<bool, ParseBoolError>)
                    .help("Whether or not the role should be joinable using `join-role` and leaveable using `leave-role`")
                    .value_name("JOINABLE")
                    .default_value("true")
                    .required(false)
                    )
                    .about("Sets a role as user joinable.")
            )
    }
);
}

///
pub struct Roles;

impl Command for Roles {
    fn invoke(&self, _disp: &Dispatch, ctx: &Context, msg: &Message, args: Cow<str>) -> super::commands::Result<()> {
        let m: ArgMatches = PARSER.with(|p|
            parse_app_matches("roles", args, &p)
        )?;

        let role = m.value_of("role-id").unwrap();
        let parsed = RoleId::from_str(role);
        let guild = msg.guild(ctx).unwrap();
        let rg = guild.read();

        let real_role = if let Ok(id) = parsed {
            rg.roles.get(&id)
        } else {
            // Maybe it's a name?
            rg.role_by_name(role)
        }.ok_or_else(|| DeniedWithReason("No such role.".into()))?;

        let reply = match m.subcommand() {
            ("set-joinable", Some(m)) => {
                let joinable = m.value_of("joinable").unwrap().parse::<bool>().unwrap();
                let c = get_cached_connection(guild.read().id)?;
                let cr = c.borrow();
                let sql = if joinable {
                    "INSERT OR IGNORE INTO joinable_roles VALUES (?);"
                } else {
                    "DELETE FROM joinable_roles WHERE role = ?;"
                };

                cr.as_ref().execute(
                    sql,
                    params![real_role.id.0 as i64]
                ).map_err(crate::db::DatabaseError::SQLError)?;

                "Role updated."
            },
            (s, Some(m)) => {
                let user = m.value_of("user-id").unwrap();
                let parsed = UserId::from_str(user);
                let real_user_id = if let Ok(id) = parsed {
                    id
                } else {
                    rg.member_named(user).ok_or_else(|| DeniedWithReason("No such user.".into()))?.user.read().id
                };

                let adding = s == "add-user";
                let mut member = rg.member(ctx, real_user_id).map_err(AnyError::boxed)?;
                if adding {
                    member.add_role(ctx, real_role.id).map_err(AnyError::boxed)?;
                    "Added role to user."
                } else {
                    member.remove_role(ctx, real_role.id).map_err(AnyError::boxed)?;
                    "Removed role from user."
                }
            },
            _ => unreachable!()
        };

        let reply = MessageBuilder::new()
            .push_codeblock_safe(reply, None)
            .build();

        msg.channel_id.say(ctx, reply).map_err(AnyError::boxed)?;

        Ok(())
    }

    fn help(&self) -> Cow<'static, str> {
        let c = PARSER.with(|p| (*p).clone());
        let e = c.get_matches_from_safe(["roles", "help"].iter());
        match e {
            Err(clap::Error { message, .. }) => {
                Cow::Owned(message)
            }
            _ => unreachable!()
        }
    }
}

/// Creates a roles [Module].
pub fn roles_module() -> Module {
    Module::with_name("roles")
        .with_sensitivity(true)
        .with_dependency("config")
        .with_config_value(config::Value::new(
            ADMIN_KEY,
            "The role which should be allowed to run restricted commands.",
            config::valid_parseable::<u64>,
            Option::<String>::None,
        ))
        .with_command_hook(role_hook)
        .with_command(Roles)
}