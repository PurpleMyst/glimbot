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

//! Contains the argument parser for commands relevant to the overall running of the Glimbot service.

use clap::{App, SubCommand, ArgMatches};
use serenity::Client;
use crate::modules::ping::ping_module;
use crate::modules::base_hooks::base_hooks;
use crate::modules::no_bot::deny_bot_mod;
use crate::modules::config::config_mod;
use crate::modules::roles::roles_module;
use crate::modules::me::me_mod;

#[doc(hidden)]
pub fn command_parser() -> App<'static, 'static> {
    SubCommand::with_name("start")
        .about("Starts the Glimbot service.")
}

#[doc(hidden)]
pub fn handle_matches(m: &ArgMatches) -> anyhow::Result<()> {
    if let ("start", Some(_)) = m.subcommand() {
        let token = std::env::var("GLIMBOT_TOKEN")?;
        let owner = std::env::var("GLIMBOT_OWNER").unwrap_or_default().parse::<u64>()?;
        let dispatch = super::Dispatch::new(owner.into())
            .with_module(base_hooks())
            .with_module(deny_bot_mod())
            .with_module(config_mod())
            .with_module(roles_module())
            .with_module(ping_module())
            .with_module(me_mod());
        let mut client = Client::new(token, dispatch)?;
        client.start_autosharded()?;
    }

    Ok(())
}