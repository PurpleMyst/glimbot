use clap::{App, SubCommand, Arg, AppSettings, ArgMatches};
use crate::db::{DB_VERSION, DatabaseVersion, DB_VERSION_STRING, DatabaseError, migrate_to, ensure_guild_db, new_conn, get_db_version, upgrade, downgrade};
use failure::{Fallible, err_msg};
use std::convert::TryFrom;
use itertools::Itertools;

pub fn command_parser() -> App<'static, 'static> {
    let arg = Arg::with_name("dbs")
        .multiple(true)
        .required(true)
        .value_name("DATABASE_FILES")
        .help("The database files to migrate up or down.");

    SubCommand::with_name("db")
        .about("Commands related to maintaining the database files for Glimbot.")
        .subcommand(SubCommand::with_name("migrate")
            .arg(arg)
            .arg(Arg::with_name("version")
                .short("V")
                .required(false)
                .takes_value(true)
                .default_value(DB_VERSION_STRING.as_str())
                .help("The desired database version.")
            )
            .arg(
                Arg::with_name("down")
                    .short("D")
                    .takes_value(false)
                    .help("Allows applying migrations to undo upgrades.")
            )
            .about("Migrates the specified database files to the latest database version, or an earlier version with --down.")
        )
        .subcommand(SubCommand::with_name("query")
            .arg(Arg::with_name("db")
                .required(true)
                .value_name("DATABASE_FILE")
                .help("The database file about which to query information. Should be {guild_id}.sqlite3.")
            )
            .about("Queries the version of a Glimbot database file.")
        )
        .setting(AppSettings::SubcommandRequiredElseHelp)
}

pub fn handle_matches(m: &ArgMatches) -> Fallible<()> {
    if let ("db", Some(m)) = m.subcommand() {
        match m.subcommand() {
            ("migrate", Some(m)) => {
                let target_version_str = m.value_of("version").unwrap();
                let tv_raw = target_version_str.parse::<u32>()? | DatabaseVersion::INITIALIZE_MASK;
                let tv = DatabaseVersion::from(tv_raw);
                let down = m.is_present("down");
                let successes = m.values_of("dbs")
                    .unwrap()
                    .map(|c| {
                        info!("Migrating {}...", c);
                        c
                    })
                    .map(new_conn)
                    .map(|c| c.and_then(|mut conn| {
                        if !down {
                            upgrade(&mut conn, Some(tv))
                        } else {
                            downgrade(&mut conn, tv)
                        }
                    }))
                    .map(|r| {
                        if let Err(e) = &r {
                            error!("Failed while migrating: {}", e)
                        }
                        r
                    })
                    .filter(Result::is_ok)
                    .count();

                info!("Successfully migrated {} databases.", successes);
            }
            ("query", Some(m)) => {
                let db = m.value_of("db").unwrap();
                let conn = new_conn(db)?;
                let ver = get_db_version(&conn)?;
                info!("Database is at version {}", ver);
            }
            _ => unreachable!()
        }
    }

    Ok(())
}