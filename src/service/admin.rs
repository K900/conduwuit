use std::{
    collections::BTreeMap,
    convert::{TryFrom, TryInto},
    sync::Arc,
    time::Instant,
};

use clap::Parser;
use regex::Regex;
use ruma::{
    events::{
        room::{
            canonical_alias::RoomCanonicalAliasEventContent,
            create::RoomCreateEventContent,
            guest_access::{GuestAccess, RoomGuestAccessEventContent},
            history_visibility::{HistoryVisibility, RoomHistoryVisibilityEventContent},
            join_rules::{JoinRule, RoomJoinRulesEventContent},
            member::{MembershipState, RoomMemberEventContent},
            message::RoomMessageEventContent,
            name::RoomNameEventContent,
            power_levels::RoomPowerLevelsEventContent,
            topic::RoomTopicEventContent,
        },
        RoomEventType,
    },
    EventId, RoomAliasId, RoomId, RoomName, RoomVersionId, ServerName, UserId,
};
use serde_json::value::to_raw_value;
use tokio::sync::{mpsc, MutexGuard, RwLock, RwLockReadGuard};

use crate::{services, Error, api::{server_server, client_server::AUTO_GEN_PASSWORD_LENGTH}, PduEvent, utils::{HtmlEscape, self}};

use super::pdu::PduBuilder;

#[derive(Debug)]
pub enum AdminRoomEvent {
    ProcessMessage(String),
    SendMessage(RoomMessageEventContent),
}

#[derive(Clone)]
pub struct Admin {
    pub sender: mpsc::UnboundedSender<AdminRoomEvent>,
}

impl Admin {
    pub fn start_handler(
        &self,
        mut receiver: mpsc::UnboundedReceiver<AdminRoomEvent>,
    ) {
        tokio::spawn(async move {
            // TODO: Use futures when we have long admin commands
            //let mut futures = FuturesUnordered::new();

            let conduit_user = UserId::parse(format!("@conduit:{}", services().globals.server_name()))
                .expect("@conduit:server_name is valid");

            let conduit_room = services()
                .rooms
                .id_from_alias(
                    format!("#admins:{}", services().globals.server_name())
                        .as_str()
                        .try_into()
                        .expect("#admins:server_name is a valid room alias"),
                )
                .expect("Database data for admin room alias must be valid")
                .expect("Admin room must exist");

            let send_message = |message: RoomMessageEventContent,
                                mutex_lock: &MutexGuard<'_, ()>| {
                services()
                    .rooms
                    .build_and_append_pdu(
                        PduBuilder {
                            event_type: RoomEventType::RoomMessage,
                            content: to_raw_value(&message)
                                .expect("event is valid, we just created it"),
                            unsigned: None,
                            state_key: None,
                            redacts: None,
                        },
                        &conduit_user,
                        &conduit_room,
                        mutex_lock,
                    )
                    .unwrap();
            };

            loop {
                tokio::select! {
                    Some(event) = receiver.recv() => {
                        let message_content = match event {
                            AdminRoomEvent::SendMessage(content) => content,
                            AdminRoomEvent::ProcessMessage(room_message) => process_admin_message(room_message).await
                        };

                        let mutex_state = Arc::clone(
                            services().globals
                                .roomid_mutex_state
                                .write()
                                .unwrap()
                                .entry(conduit_room.clone())
                                .or_default(),
                        );

                        let state_lock = mutex_state.lock().await;

                        send_message(message_content, &state_lock);

                        drop(state_lock);
                    }
                }
            }
        });
    }

    pub fn process_message(&self, room_message: String) {
        self.sender
            .send(AdminRoomEvent::ProcessMessage(room_message))
            .unwrap();
    }

    pub fn send_message(&self, message_content: RoomMessageEventContent) {
        self.sender
            .send(AdminRoomEvent::SendMessage(message_content))
            .unwrap();
    }
}

// Parse and process a message from the admin room
async fn process_admin_message(room_message: String) -> RoomMessageEventContent {
    let mut lines = room_message.lines();
    let command_line = lines.next().expect("each string has at least one line");
    let body: Vec<_> = lines.collect();

    let admin_command = match parse_admin_command(&command_line) {
        Ok(command) => command,
        Err(error) => {
            let server_name = services().globals.server_name();
            let message = error
                .to_string()
                .replace("server.name", server_name.as_str());
            let html_message = usage_to_html(&message, server_name);

            return RoomMessageEventContent::text_html(message, html_message);
        }
    };

    match process_admin_command(admin_command, body).await {
        Ok(reply_message) => reply_message,
        Err(error) => {
            let markdown_message = format!(
                "Encountered an error while handling the command:\n\
                ```\n{}\n```",
                error,
            );
            let html_message = format!(
                "Encountered an error while handling the command:\n\
                <pre>\n{}\n</pre>",
                error,
            );

            RoomMessageEventContent::text_html(markdown_message, html_message)
        }
    }
}

// Parse chat messages from the admin room into an AdminCommand object
fn parse_admin_command(command_line: &str) -> std::result::Result<AdminCommand, String> {
    // Note: argv[0] is `@conduit:servername:`, which is treated as the main command
    let mut argv: Vec<_> = command_line.split_whitespace().collect();

    // Replace `help command` with `command --help`
    // Clap has a help subcommand, but it omits the long help description.
    if argv.len() > 1 && argv[1] == "help" {
        argv.remove(1);
        argv.push("--help");
    }

    // Backwards compatibility with `register_appservice`-style commands
    let command_with_dashes;
    if argv.len() > 1 && argv[1].contains("_") {
        command_with_dashes = argv[1].replace("_", "-");
        argv[1] = &command_with_dashes;
    }

    AdminCommand::try_parse_from(argv).map_err(|error| error.to_string())
}

#[derive(Parser)]
#[clap(name = "@conduit:server.name:", version = env!("CARGO_PKG_VERSION"))]
enum AdminCommand {
    #[clap(verbatim_doc_comment)]
    /// Register an appservice using its registration YAML
    ///
    /// This command needs a YAML generated by an appservice (such as a bridge),
    /// which must be provided in a Markdown code-block below the command.
    ///
    /// Registering a new bridge using the ID of an existing bridge will replace
    /// the old one.
    ///
    /// [commandbody]
    /// # ```
    /// # yaml content here
    /// # ```
    RegisterAppservice,

    /// Unregister an appservice using its ID
    ///
    /// You can find the ID using the `list-appservices` command.
    UnregisterAppservice {
        /// The appservice to unregister
        appservice_identifier: String,
    },

    /// List all the currently registered appservices
    ListAppservices,

    /// List all rooms the server knows about
    ListRooms,

    /// List users in the database
    ListLocalUsers,

    /// List all rooms we are currently handling an incoming pdu from
    IncomingFederation,

    /// Deactivate a user
    ///
    /// User will not be removed from all rooms by default.
    /// Use --leave-rooms to force the user to leave all rooms
    DeactivateUser {
        #[clap(short, long)]
        leave_rooms: bool,
        user_id: Box<UserId>,
    },

    #[clap(verbatim_doc_comment)]
    /// Deactivate a list of users
    ///
    /// Recommended to use in conjunction with list-local-users.
    ///
    /// Users will not be removed from joined rooms by default.
    /// Can be overridden with --leave-rooms flag.
    /// Removing a mass amount of users from a room may cause a significant amount of leave events.
    /// The time to leave rooms may depend significantly on joined rooms and servers.
    ///
    /// [commandbody]
    /// # ```
    /// # User list here
    /// # ```
    DeactivateAll {
        #[clap(short, long)]
        /// Remove users from their joined rooms
        leave_rooms: bool,
        #[clap(short, long)]
        /// Also deactivate admin accounts
        force: bool,
    },

    /// Get the auth_chain of a PDU
    GetAuthChain {
        /// An event ID (the $ character followed by the base64 reference hash)
        event_id: Box<EventId>,
    },

    #[clap(verbatim_doc_comment)]
    /// Parse and print a PDU from a JSON
    ///
    /// The PDU event is only checked for validity and is not added to the
    /// database.
    ///
    /// [commandbody]
    /// # ```
    /// # PDU json content here
    /// # ```
    ParsePdu,

    /// Retrieve and print a PDU by ID from the Conduit database
    GetPdu {
        /// An event ID (a $ followed by the base64 reference hash)
        event_id: Box<EventId>,
    },

    /// Print database memory usage statistics
    DatabaseMemoryUsage,

    /// Show configuration values
    ShowConfig,

    /// Reset user password
    ResetPassword {
        /// Username of the user for whom the password should be reset
        username: String,
    },

    /// Create a new user
    CreateUser {
        /// Username of the new user
        username: String,
        /// Password of the new user, if unspecified one is generated
        password: Option<String>,
    },

    /// Disables incoming federation handling for a room.
    DisableRoom { room_id: Box<RoomId> },
    /// Enables incoming federation handling for a room again.
    EnableRoom { room_id: Box<RoomId> },
}

async fn process_admin_command(
    command: AdminCommand,
    body: Vec<&str>,
) -> Result<RoomMessageEventContent> {
    let reply_message_content = match command {
        AdminCommand::RegisterAppservice => {
            if body.len() > 2 && body[0].trim() == "```" && body.last().unwrap().trim() == "```" {
                let appservice_config = body[1..body.len() - 1].join("\n");
                let parsed_config = serde_yaml::from_str::<serde_yaml::Value>(&appservice_config);
                match parsed_config {
                    Ok(yaml) => match services().appservice.register_appservice(yaml) {
                        Ok(id) => RoomMessageEventContent::text_plain(format!(
                            "Appservice registered with ID: {}.",
                            id
                        )),
                        Err(e) => RoomMessageEventContent::text_plain(format!(
                            "Failed to register appservice: {}",
                            e
                        )),
                    },
                    Err(e) => RoomMessageEventContent::text_plain(format!(
                        "Could not parse appservice config: {}",
                        e
                    )),
                }
            } else {
                RoomMessageEventContent::text_plain(
                    "Expected code block in command body. Add --help for details.",
                )
            }
        }
        AdminCommand::UnregisterAppservice {
            appservice_identifier,
        } => match services().appservice.unregister_appservice(&appservice_identifier) {
            Ok(()) => RoomMessageEventContent::text_plain("Appservice unregistered."),
            Err(e) => RoomMessageEventContent::text_plain(format!(
                "Failed to unregister appservice: {}",
                e
            )),
        },
        AdminCommand::ListAppservices => {
            if let Ok(appservices) = services().appservice.iter_ids().map(|ids| ids.collect::<Vec<_>>()) {
                let count = appservices.len();
                let output = format!(
                    "Appservices ({}): {}",
                    count,
                    appservices
                        .into_iter()
                        .filter_map(|r| r.ok())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                RoomMessageEventContent::text_plain(output)
            } else {
                RoomMessageEventContent::text_plain("Failed to get appservices.")
            }
        }
        AdminCommand::ListRooms => {
            let room_ids = services().rooms.iter_ids();
            let output = format!(
                "Rooms:\n{}",
                room_ids
                    .filter_map(|r| r.ok())
                    .map(|id| id.to_string()
                        + "\tMembers: "
                        + &services()
                            .rooms
                            .room_joined_count(&id)
                            .ok()
                            .flatten()
                            .unwrap_or(0)
                            .to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            RoomMessageEventContent::text_plain(output)
        }
        AdminCommand::ListLocalUsers => match services().users.list_local_users() {
            Ok(users) => {
                let mut msg: String = format!("Found {} local user account(s):\n", users.len());
                msg += &users.join("\n");
                RoomMessageEventContent::text_plain(&msg)
            }
            Err(e) => RoomMessageEventContent::text_plain(e.to_string()),
        },
        AdminCommand::IncomingFederation => {
            let map = services().globals.roomid_federationhandletime.read().unwrap();
            let mut msg: String = format!("Handling {} incoming pdus:\n", map.len());

            for (r, (e, i)) in map.iter() {
                let elapsed = i.elapsed();
                msg += &format!(
                    "{} {}: {}m{}s\n",
                    r,
                    e,
                    elapsed.as_secs() / 60,
                    elapsed.as_secs() % 60
                );
            }
            RoomMessageEventContent::text_plain(&msg)
        }
        AdminCommand::GetAuthChain { event_id } => {
            let event_id = Arc::<EventId>::from(event_id);
            if let Some(event) = services().rooms.get_pdu_json(&event_id)? {
                let room_id_str = event
                    .get("room_id")
                    .and_then(|val| val.as_str())
                    .ok_or_else(|| Error::bad_database("Invalid event in database"))?;

                let room_id = <&RoomId>::try_from(room_id_str).map_err(|_| {
                    Error::bad_database("Invalid room id field in event in database")
                })?;
                let start = Instant::now();
                let count = server_server::get_auth_chain(room_id, vec![event_id])
                    .await?
                    .count();
                let elapsed = start.elapsed();
                RoomMessageEventContent::text_plain(format!(
                    "Loaded auth chain with length {} in {:?}",
                    count, elapsed
                ))
            } else {
                RoomMessageEventContent::text_plain("Event not found.")
            }
        }
        AdminCommand::ParsePdu => {
            if body.len() > 2 && body[0].trim() == "```" && body.last().unwrap().trim() == "```" {
                let string = body[1..body.len() - 1].join("\n");
                match serde_json::from_str(&string) {
                    Ok(value) => {
                        match ruma::signatures::reference_hash(&value, &RoomVersionId::V6) {
                            Ok(hash) => {
                                let event_id = EventId::parse(format!("${}", hash));

                                match serde_json::from_value::<PduEvent>(
                                    serde_json::to_value(value).expect("value is json"),
                                ) {
                                    Ok(pdu) => RoomMessageEventContent::text_plain(format!(
                                        "EventId: {:?}\n{:#?}",
                                        event_id, pdu
                                    )),
                                    Err(e) => RoomMessageEventContent::text_plain(format!(
                                        "EventId: {:?}\nCould not parse event: {}",
                                        event_id, e
                                    )),
                                }
                            }
                            Err(e) => RoomMessageEventContent::text_plain(format!(
                                "Could not parse PDU JSON: {:?}",
                                e
                            )),
                        }
                    }
                    Err(e) => RoomMessageEventContent::text_plain(format!(
                        "Invalid json in command body: {}",
                        e
                    )),
                }
            } else {
                RoomMessageEventContent::text_plain("Expected code block in command body.")
            }
        }
        AdminCommand::GetPdu { event_id } => {
            let mut outlier = false;
            let mut pdu_json = services().rooms.get_non_outlier_pdu_json(&event_id)?;
            if pdu_json.is_none() {
                outlier = true;
                pdu_json = services().rooms.get_pdu_json(&event_id)?;
            }
            match pdu_json {
                Some(json) => {
                    let json_text =
                        serde_json::to_string_pretty(&json).expect("canonical json is valid json");
                    RoomMessageEventContent::text_html(
                        format!(
                            "{}\n```json\n{}\n```",
                            if outlier {
                                "PDU is outlier"
                            } else {
                                "PDU was accepted"
                            },
                            json_text
                        ),
                        format!(
                            "<p>{}</p>\n<pre><code class=\"language-json\">{}\n</code></pre>\n",
                            if outlier {
                                "PDU is outlier"
                            } else {
                                "PDU was accepted"
                            },
                            HtmlEscape(&json_text)
                        ),
                    )
                }
                None => RoomMessageEventContent::text_plain("PDU not found."),
            }
        }
        AdminCommand::DatabaseMemoryUsage => match services()._db.memory_usage() {
            Ok(response) => RoomMessageEventContent::text_plain(response),
            Err(e) => RoomMessageEventContent::text_plain(format!(
                "Failed to get database memory usage: {}",
                e
            )),
        },
        AdminCommand::ShowConfig => {
            // Construct and send the response
            RoomMessageEventContent::text_plain(format!("{}", services().globals.config))
        }
        AdminCommand::ResetPassword { username } => {
            let user_id = match UserId::parse_with_server_name(
                username.as_str().to_lowercase(),
                services().globals.server_name(),
            ) {
                Ok(id) => id,
                Err(e) => {
                    return Ok(RoomMessageEventContent::text_plain(format!(
                        "The supplied username is not a valid username: {}",
                        e
                    )))
                }
            };

            // Check if the specified user is valid
            if !services().users.exists(&user_id)?
                || services().users.is_deactivated(&user_id)?
                || user_id
                    == UserId::parse_with_server_name("conduit", services().globals.server_name())
                        .expect("conduit user exists")
            {
                return Ok(RoomMessageEventContent::text_plain(
                    "The specified user does not exist or is deactivated!",
                ));
            }

            let new_password = utils::random_string(AUTO_GEN_PASSWORD_LENGTH);

            match services().users.set_password(&user_id, Some(new_password.as_str())) {
                Ok(()) => RoomMessageEventContent::text_plain(format!(
                    "Successfully reset the password for user {}: {}",
                    user_id, new_password
                )),
                Err(e) => RoomMessageEventContent::text_plain(format!(
                    "Couldn't reset the password for user {}: {}",
                    user_id, e
                )),
            }
        }
        AdminCommand::CreateUser { username, password } => {
            let password = password.unwrap_or(utils::random_string(AUTO_GEN_PASSWORD_LENGTH));
            // Validate user id
            let user_id = match UserId::parse_with_server_name(
                username.as_str().to_lowercase(),
                services().globals.server_name(),
            ) {
                Ok(id) => id,
                Err(e) => {
                    return Ok(RoomMessageEventContent::text_plain(format!(
                        "The supplied username is not a valid username: {}",
                        e
                    )))
                }
            };
            if user_id.is_historical() {
                return Ok(RoomMessageEventContent::text_plain(format!(
                    "userid {user_id} is not allowed due to historical"
                )));
            }
            if services().users.exists(&user_id)? {
                return Ok(RoomMessageEventContent::text_plain(format!(
                    "userid {user_id} already exists"
                )));
            }
            // Create user
            services().users.create(&user_id, Some(password.as_str()))?;

            // Default to pretty displayname
            let displayname = format!("{} ⚡️", user_id.localpart());
            services().users
                .set_displayname(&user_id, Some(displayname.clone()))?;

            // Initial account data
            services().account_data.update(
                None,
                &user_id,
                ruma::events::GlobalAccountDataEventType::PushRules
                    .to_string()
                    .into(),
                &ruma::events::push_rules::PushRulesEvent {
                    content: ruma::events::push_rules::PushRulesEventContent {
                        global: ruma::push::Ruleset::server_default(&user_id),
                    },
                },
            )?;

            // we dont add a device since we're not the user, just the creator

            // Inhibit login does not work for guests
            RoomMessageEventContent::text_plain(format!(
                "Created user with user_id: {user_id} and password: {password}"
            ))
        }
        AdminCommand::DisableRoom { room_id } => {
            services().rooms.disabledroomids.insert(room_id.as_bytes(), &[])?;
            RoomMessageEventContent::text_plain("Room disabled.")
        }
        AdminCommand::EnableRoom { room_id } => {
            services().rooms.disabledroomids.remove(room_id.as_bytes())?;
            RoomMessageEventContent::text_plain("Room enabled.")
        }
        AdminCommand::DeactivateUser {
            leave_rooms,
            user_id,
        } => {
            let user_id = Arc::<UserId>::from(user_id);
            if services().users.exists(&user_id)? {
                RoomMessageEventContent::text_plain(format!(
                    "Making {} leave all rooms before deactivation...",
                    user_id
                ));

                services().users.deactivate_account(&user_id)?;

                if leave_rooms {
                    services().rooms.leave_all_rooms(&user_id).await?;
                }

                RoomMessageEventContent::text_plain(format!(
                    "User {} has been deactivated",
                    user_id
                ))
            } else {
                RoomMessageEventContent::text_plain(format!(
                    "User {} doesn't exist on this server",
                    user_id
                ))
            }
        }
        AdminCommand::DeactivateAll { leave_rooms, force } => {
            if body.len() > 2 && body[0].trim() == "```" && body.last().unwrap().trim() == "```" {
                let usernames = body.clone().drain(1..body.len() - 1).collect::<Vec<_>>();

                let mut user_ids: Vec<&UserId> = Vec::new();

                for &username in &usernames {
                    match <&UserId>::try_from(username) {
                        Ok(user_id) => user_ids.push(user_id),
                        Err(_) => {
                            return Ok(RoomMessageEventContent::text_plain(format!(
                                "{} is not a valid username",
                                username
                            )))
                        }
                    }
                }

                let mut deactivation_count = 0;
                let mut admins = Vec::new();

                if !force {
                    user_ids.retain(|&user_id| {
                        match services().users.is_admin(user_id) {
                            Ok(is_admin) => match is_admin {
                                true => {
                                    admins.push(user_id.localpart());
                                    false
                                }
                                false => true,
                            },
                            Err(_) => false,
                        }
                    })
                }

                for &user_id in &user_ids {
                    match services().users.deactivate_account(user_id) {
                        Ok(_) => deactivation_count += 1,
                        Err(_) => {}
                    }
                }

                if leave_rooms {
                    for &user_id in &user_ids {
                        let _ = services().rooms.leave_all_rooms(user_id).await;
                    }
                }

                if admins.is_empty() {
                    RoomMessageEventContent::text_plain(format!(
                        "Deactivated {} accounts.",
                        deactivation_count
                    ))
                } else {
                    RoomMessageEventContent::text_plain(format!("Deactivated {} accounts.\nSkipped admin accounts: {:?}. Use --force to deactivate admin accounts", deactivation_count, admins.join(", ")))
                }
            } else {
                RoomMessageEventContent::text_plain(
                    "Expected code block in command body. Add --help for details.",
                )
            }
        }
    };

    Ok(reply_message_content)
}

// Utility to turn clap's `--help` text to HTML.
fn usage_to_html(text: &str, server_name: &ServerName) -> String {
    // Replace `@conduit:servername:-subcmdname` with `@conduit:servername: subcmdname`
    let text = text.replace(
        &format!("@conduit:{}:-", server_name),
        &format!("@conduit:{}: ", server_name),
    );

    // For the conduit admin room, subcommands become main commands
    let text = text.replace("SUBCOMMAND", "COMMAND");
    let text = text.replace("subcommand", "command");

    // Escape option names (e.g. `<element-id>`) since they look like HTML tags
    let text = text.replace("<", "&lt;").replace(">", "&gt;");

    // Italicize the first line (command name and version text)
    let re = Regex::new("^(.*?)\n").expect("Regex compilation should not fail");
    let text = re.replace_all(&text, "<em>$1</em>\n");

    // Unmerge wrapped lines
    let text = text.replace("\n            ", "  ");

    // Wrap option names in backticks. The lines look like:
    //     -V, --version  Prints version information
    // And are converted to:
    // <code>-V, --version</code>: Prints version information
    // (?m) enables multi-line mode for ^ and $
    let re = Regex::new("(?m)^    (([a-zA-Z_&;-]+(, )?)+)  +(.*)$")
        .expect("Regex compilation should not fail");
    let text = re.replace_all(&text, "<code>$1</code>: $4");

    // Look for a `[commandbody]` tag. If it exists, use all lines below it that
    // start with a `#` in the USAGE section.
    let mut text_lines: Vec<&str> = text.lines().collect();
    let mut command_body = String::new();

    if let Some(line_index) = text_lines.iter().position(|line| *line == "[commandbody]") {
        text_lines.remove(line_index);

        while text_lines
            .get(line_index)
            .map(|line| line.starts_with("#"))
            .unwrap_or(false)
        {
            command_body += if text_lines[line_index].starts_with("# ") {
                &text_lines[line_index][2..]
            } else {
                &text_lines[line_index][1..]
            };
            command_body += "[nobr]\n";
            text_lines.remove(line_index);
        }
    }

    let text = text_lines.join("\n");

    // Improve the usage section
    let text = if command_body.is_empty() {
        // Wrap the usage line in code tags
        let re = Regex::new("(?m)^USAGE:\n    (@conduit:.*)$")
            .expect("Regex compilation should not fail");
        re.replace_all(&text, "USAGE:\n<code>$1</code>").to_string()
    } else {
        // Wrap the usage line in a code block, and add a yaml block example
        // This makes the usage of e.g. `register-appservice` more accurate
        let re =
            Regex::new("(?m)^USAGE:\n    (.*?)\n\n").expect("Regex compilation should not fail");
        re.replace_all(&text, "USAGE:\n<pre>$1[nobr]\n[commandbodyblock]</pre>")
            .replace("[commandbodyblock]", &command_body)
    };

    // Add HTML line-breaks
    let text = text
        .replace("\n\n\n", "\n\n")
        .replace("\n", "<br>\n")
        .replace("[nobr]<br>", "");

    text
}

/// Create the admin room.
///
/// Users in this room are considered admins by conduit, and the room can be
/// used to issue admin commands by talking to the server user inside it.
pub(crate) async fn create_admin_room() -> Result<()> {
    let room_id = RoomId::new(services().globals.server_name());

    services().rooms.get_or_create_shortroomid(&room_id)?;

    let mutex_state = Arc::clone(
        services().globals
            .roomid_mutex_state
            .write()
            .unwrap()
            .entry(room_id.clone())
            .or_default(),
    );
    let state_lock = mutex_state.lock().await;

    // Create a user for the server
    let conduit_user = UserId::parse_with_server_name("conduit", services().globals.server_name())
        .expect("@conduit:server_name is valid");

    services().users.create(&conduit_user, None)?;

    let mut content = RoomCreateEventContent::new(conduit_user.clone());
    content.federate = true;
    content.predecessor = None;
    content.room_version = RoomVersionId::V6;

    // 1. The room create event
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomCreate,
            content: to_raw_value(&content).expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 2. Make conduit bot join
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomMember,
            content: to_raw_value(&RoomMemberEventContent {
                membership: MembershipState::Join,
                displayname: None,
                avatar_url: None,
                is_direct: None,
                third_party_invite: None,
                blurhash: None,
                reason: None,
                join_authorized_via_users_server: None,
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some(conduit_user.to_string()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 3. Power levels
    let mut users = BTreeMap::new();
    users.insert(conduit_user.clone(), 100.into());

    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomPowerLevels,
            content: to_raw_value(&RoomPowerLevelsEventContent {
                users,
                ..Default::default()
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 4.1 Join Rules
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomJoinRules,
            content: to_raw_value(&RoomJoinRulesEventContent::new(JoinRule::Invite))
                .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 4.2 History Visibility
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomHistoryVisibility,
            content: to_raw_value(&RoomHistoryVisibilityEventContent::new(
                HistoryVisibility::Shared,
            ))
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 4.3 Guest Access
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomGuestAccess,
            content: to_raw_value(&RoomGuestAccessEventContent::new(GuestAccess::Forbidden))
                .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 5. Events implied by name and topic
    let room_name = RoomName::parse(format!("{} Admin Room", services().globals.server_name()))
        .expect("Room name is valid");
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomName,
            content: to_raw_value(&RoomNameEventContent::new(Some(room_name)))
                .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomTopic,
            content: to_raw_value(&RoomTopicEventContent {
                topic: format!("Manage {}", services().globals.server_name()),
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // 6. Room alias
    let alias: Box<RoomAliasId> = format!("#admins:{}", services().globals.server_name())
        .try_into()
        .expect("#admins:server_name is a valid alias name");

    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomCanonicalAlias,
            content: to_raw_value(&RoomCanonicalAliasEventContent {
                alias: Some(alias.clone()),
                alt_aliases: Vec::new(),
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    services().rooms.set_alias(&alias, Some(&room_id))?;

    Ok(())
}

/// Invite the user to the conduit admin room.
///
/// In conduit, this is equivalent to granting admin privileges.
pub(crate) async fn make_user_admin(
    user_id: &UserId,
    displayname: String,
) -> Result<()> {
    let admin_room_alias: Box<RoomAliasId> = format!("#admins:{}", services().globals.server_name())
        .try_into()
        .expect("#admins:server_name is a valid alias name");
    let room_id = services()
        .rooms
        .id_from_alias(&admin_room_alias)?
        .expect("Admin room must exist");

    let mutex_state = Arc::clone(
        services().globals
            .roomid_mutex_state
            .write()
            .unwrap()
            .entry(room_id.clone())
            .or_default(),
    );
    let state_lock = mutex_state.lock().await;

    // Use the server user to grant the new admin's power level
    let conduit_user = UserId::parse_with_server_name("conduit", services().globals.server_name())
        .expect("@conduit:server_name is valid");

    // Invite and join the real user
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomMember,
            content: to_raw_value(&RoomMemberEventContent {
                membership: MembershipState::Invite,
                displayname: None,
                avatar_url: None,
                is_direct: None,
                third_party_invite: None,
                blurhash: None,
                reason: None,
                join_authorized_via_users_server: None,
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some(user_id.to_string()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomMember,
            content: to_raw_value(&RoomMemberEventContent {
                membership: MembershipState::Join,
                displayname: Some(displayname),
                avatar_url: None,
                is_direct: None,
                third_party_invite: None,
                blurhash: None,
                reason: None,
                join_authorized_via_users_server: None,
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some(user_id.to_string()),
            redacts: None,
        },
        &user_id,
        &room_id,
        &state_lock,
    )?;

    // Set power level
    let mut users = BTreeMap::new();
    users.insert(conduit_user.to_owned(), 100.into());
    users.insert(user_id.to_owned(), 100.into());

    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomPowerLevels,
            content: to_raw_value(&RoomPowerLevelsEventContent {
                users,
                ..Default::default()
            })
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: Some("".to_owned()),
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    // Send welcome message
    services().rooms.build_and_append_pdu(
        PduBuilder {
            event_type: RoomEventType::RoomMessage,
            content: to_raw_value(&RoomMessageEventContent::text_html(
                    format!("## Thank you for trying out Conduit!\n\nConduit is currently in Beta. This means you can join and participate in most Matrix rooms, but not all features are supported and you might run into bugs from time to time.\n\nHelpful links:\n> Website: https://conduit.rs\n> Git and Documentation: https://gitlab.com/famedly/conduit\n> Report issues: https://gitlab.com/famedly/conduit/-/issues\n\nFor a list of available commands, send the following message in this room: `@conduit:{}: --help`\n\nHere are some rooms you can join (by typing the command):\n\nConduit room (Ask questions and get notified on updates):\n`/join #conduit:fachschaften.org`\n\nConduit lounge (Off-topic, only Conduit users are allowed to join)\n`/join #conduit-lounge:conduit.rs`", services().globals.server_name()).to_owned(),
                    format!("<h2>Thank you for trying out Conduit!</h2>\n<p>Conduit is currently in Beta. This means you can join and participate in most Matrix rooms, but not all features are supported and you might run into bugs from time to time.</p>\n<p>Helpful links:</p>\n<blockquote>\n<p>Website: https://conduit.rs<br>Git and Documentation: https://gitlab.com/famedly/conduit<br>Report issues: https://gitlab.com/famedly/conduit/-/issues</p>\n</blockquote>\n<p>For a list of available commands, send the following message in this room: <code>@conduit:{}: --help</code></p>\n<p>Here are some rooms you can join (by typing the command):</p>\n<p>Conduit room (Ask questions and get notified on updates):<br><code>/join #conduit:fachschaften.org</code></p>\n<p>Conduit lounge (Off-topic, only Conduit users are allowed to join)<br><code>/join #conduit-lounge:conduit.rs</code></p>\n", services().globals.server_name()).to_owned(),
            ))
            .expect("event is valid, we just created it"),
            unsigned: None,
            state_key: None,
            redacts: None,
        },
        &conduit_user,
        &room_id,
        &state_lock,
    )?;

    Ok(())
}
