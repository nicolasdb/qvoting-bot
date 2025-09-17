#[macro_use]
extern crate const_format;

use dotenv::dotenv;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::all::{
    GatewayIntents, Interaction, Message, GuildId, UserId, Ready,
    CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateInteractionResponseFollowup, EditMessage,
    CommandOptionType, CommandInteraction,
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Where the discord api key should be stored in the process or .env environment
/// variables
const SECRET_KEY: &str = "DISCORD_SECRET";

/// Role name identifying users with the ability to start and stop elections.
const BOT_ROLE: &str = "voting";

/// The channel where the bot should post announcements
const BOT_CHANNEL: &str = "announcements";

/// The number of winners that should be displayed for convenience purposes
const CONVENIENT_WINNERS: usize = 5;

/// Everyone starts out with 100 points, and they reset on the below interval:
const STARTING_POINTS: usize = 100;

/// The number of hours that people can suggest ideas for
const SUGG_INTERVAL: u64 = 48;

/// The number of hours that a vote should last
const VOTE_INTERVAL: u64 = 24;

/// The bot uses slash commands exclusively

/// Environment variable name for approved servers list
const APPROVED_SERVERS_KEY: &str = "APPROVED_SERVERS";

// Make an announcement in the bot channel with comprehensive error handling
macro_rules! announce {
    ($context:expr,$guild_id:expr,$content:expr) => {{
        async {
            // Quick cache access with timeout protection
            let channel_id = match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                async {
                    $context.cache.guild($guild_id)
                        .and_then(|guild| guild.channels.iter().find(|(_, ch)| ch.name == BOT_CHANNEL).map(|(id, _)| *id))
                }
            ).await {
                Ok(Some(id)) => id,
                Ok(None) => {
                    eprintln!("Announcement channel '{}' not found in guild {}", BOT_CHANNEL, $guild_id);
                    return None;
                },
                Err(_) => {
                    eprintln!("Timeout accessing guild cache for announcement in {}", $guild_id);
                    return None;
                }
            };

            // Send message with timeout and retry logic
            let mut attempts = 0;
            const MAX_ATTEMPTS: u8 = 2;

            while attempts < MAX_ATTEMPTS {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(8),
                    channel_id.say($context, $content)
                ).await {
                    Ok(Ok(message)) => {
                        println!("Successfully sent announcement to guild {}", $guild_id);
                        return Some(message);
                    },
                    Ok(Err(e)) => {
                        eprintln!("Discord API error in announce (attempt {}): {}", attempts + 1, e);
                        if attempts + 1 >= MAX_ATTEMPTS {
                            return None;
                        }
                    },
                    Err(_) => {
                        eprintln!("Announce timeout (attempt {}) - Discord API took too long", attempts + 1);
                        if attempts + 1 >= MAX_ATTEMPTS {
                            return None;
                        }
                    }
                }
                attempts += 1;
                // Brief delay before retry
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            None
        }.await
    }};
}

// Enhanced permission checking for admin commands
macro_rules! check_admin_permission {
    ($context:expr,$guild_id:expr,$user_id:expr) => {{
        match $context.cache.guild($guild_id) {
            Some(guild) => {
                // Check if user is guild owner (always has permission)
                if guild.owner_id == $user_id.id {
                    true
                } else if let Some(member) = guild.members.get(&$user_id.id) {
                    // Check for Administrator permission
                    if let Ok(permissions) = member.permissions($context) {
                        if permissions.administrator() {
                            true
                        } else {
                            // Check for the specific voting role
                            guild.role_by_name(BOT_ROLE)
                                .map(|role| member.roles.contains(&role.id))
                                .unwrap_or(false)
                        }
                    } else {
                        // Fallback to role check only
                        guild.role_by_name(BOT_ROLE)
                            .map(|role| member.roles.contains(&role.id))
                            .unwrap_or(false)
                    }
                } else {
                    false
                }
            },
            None => false,
        }
    }};
}

/// Possible slash commands for the quadratic voting bot:
/// /prop <topic>: Adds a topic to the upcoming election
/// /vote <votes> <candidate_id>: Cast votes for the selected candidate
/// /points: Get the sender's remaining points in the election
/// /start <prompt>: Starts a new vote (can only be called by users with admin permissions)
/// /stop: Stops the segment of the voting process (can only be called by users with admin permissions)
#[derive(Default)]
struct Handler {
    // Suggested topics for the upcoming election
    upcoming_topics: HashMap<GuildId, Arc<RwLock<Vec<String>>>>,

    // Users cannot have less than 0 points, but they may have different
    // balances per-guild
    points: HashMap<GuildId, Arc<RwLock<HashMap<UserId, AtomicUsize>>>>,

    // The bot automatically updates results of the election as it progresses
    results: Arc<RwLock<HashMap<GuildId, Message>>>,

    // Total votes per idea, and votes cast per idea per user
    votes: HashMap<
        GuildId,
        Arc<RwLock<HashMap<usize, (String, AtomicUsize, HashMap<UserId, AtomicUsize>)>>>,
    >,

    // Rate limiting: track last command usage per user per guild
    last_command_time: Arc<RwLock<HashMap<(GuildId, UserId), Instant>>>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("Bot logged in as {}", ready.user.name);

        // Create modern slash commands with proper builders
        let commands = vec![
            CreateCommand::new("prop")
                .description("Propose a candidate for the election")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "idea",
                        "Your proposal"
                    )
                    .required(true)
                ),
            CreateCommand::new("vote")
                .description("Cast votes for a candidate")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "n",
                        "Votes to cast (1-10)"
                    )
                    .required(true)
                    .min_int_value(1)
                    .max_int_value(10)
                )
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "id",
                        "Candidate ID"
                    )
                    .required(true)
                    .min_int_value(0)
                ),
            CreateCommand::new("points")
                .description("Check your remaining voice credits"),
            CreateCommand::new("start")
                .description("Start a new election (requires voting role)")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "prompt",
                        "Election topic/question"
                    )
                    .required(true)
                ),
            CreateCommand::new("stop")
                .description("Stop the current election phase (requires voting role)"),
        ];

        // Register commands globally for all guilds
        match ctx.http.create_global_commands(&commands).await {
            Ok(_) => println!("Successfully registered {} global slash commands", commands.len()),
            Err(why) => println!("Failed to register global commands: {:?}", why),
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction {
            println!("Received slash command: {} from user: {}", command.data.name, command.user.id);

            // Handle commands with appropriate response patterns
            match command.data.name.as_str() {
                "prop" => {
                    self.handle_prop_command(&ctx, &command).await;
                },
                "vote" => {
                    self.handle_vote_command(&ctx, &command).await;
                },
                "points" => {
                    self.handle_points_command(&ctx, &command).await;
                },
                "start" => {
                    self.handle_start_command(&ctx, &command).await;
                },
                "stop" => {
                    self.handle_stop_command(&ctx, &command).await;
                },
                _ => {
                    self.send_ephemeral_response(&ctx, &command, "❌ Unknown command. Please try again.").await;
                },
            }
        }
    }
}

impl Handler {
    /// Creates buckets for all of the pre-specified servers the bot belongs to.
    fn register_servers(mut self, approved_servers: Vec<GuildId>) -> Self {
        for g in approved_servers {
            self.upcoming_topics
                .insert(g, Arc::new(RwLock::new(Vec::new())));
            self.points.insert(g, Arc::new(RwLock::new(HashMap::new())));
            self.votes.insert(g, Arc::new(RwLock::new(HashMap::new())));
        }

        self
    }

    /// Check if user is rate limited (max 1 command per 2 seconds)
    async fn check_rate_limit(&self, guild_id: GuildId, user_id: UserId) -> bool {
        let key = (guild_id, user_id);
        let now = Instant::now();
        let cooldown = Duration::from_secs(2);

        let mut times = self.last_command_time.write().await;
        if let Some(last_time) = times.get(&key) {
            if now.duration_since(*last_time) < cooldown {
                return true; // Rate limited
            }
        }
        times.insert(key, now);
        false // Not rate limited
    }






    /// Get a list of the candidates that are winning so far, sorted by their
    /// number of votes.
    async fn winners(&self, g: &GuildId) -> Vec<String> {
        // Sort the candidates, and take the first CONVENIENT_WINNERS ones
        let mut candidates = self
            .votes
            .get(g)
            .unwrap()
            .read()
            .await
            .values()
            .map(|(c, votes, _)| (c.clone(), votes.load(Ordering::Relaxed)))
            .collect::<Vec<(String, usize)>>();
        candidates.sort_by(|b, a| a.1.partial_cmp(&b.1).unwrap());
        candidates
            .iter()
            .map(|w| format!("{}: {}", w.0, w.1))
            .take(CONVENIENT_WINNERS)
            .collect::<Vec<String>>()
    }

    /// Updates the most recent announcement in the given guild with the latest suggestions.
    async fn poll_suggestions(&self, context: &Context, g: &GuildId) {
        let suggestions = self
            .upcoming_topics
            .get(g)
            .unwrap()
            .read()
            .await
            .iter()
            .map(|s| format!("• {}", s))
            .collect::<Vec<String>>();

        let cts = self
            .results
            .read()
            .await
            .get(g)
            .unwrap()
            .content
            .split_inclusive("**Suggestions so Far:**")
            .map(|s| s.to_owned())
            .next()
            .unwrap_or_default();

        self.results
            .write()
            .await
            .get_mut(g)
            .unwrap()
            .edit(context, EditMessage::new().content(format!("{}\n{}", cts, suggestions.join("\n"))))
            .await
            .expect("discord API error");
    }

    /// Safe version of poll_suggestions with proper error handling
    async fn poll_suggestions_safe(&self, context: &Context, g: &GuildId) -> Result<(), String> {
        let Some(topics_lock) = self.upcoming_topics.get(g) else {
            return Err("Guild not found in topics".to_string());
        };

        // Quick check if there's an active election to update
        {
            let results_read = self.results.read().await;
            if results_read.get(g).is_none() {
                return Err("No active election to update".to_string());
            }
        }

        let suggestions = topics_lock
            .read()
            .await
            .iter()
            .enumerate()
            .map(|(i, s)| format!("#{}: {}", i + 1, s))
            .collect::<Vec<String>>();

        let base_content = {
            let results_read = self.results.read().await;
            let Some(result_msg) = results_read.get(g) else {
                return Err("No active announcement message".to_string());
            };
            let content_parts: Vec<&str> = result_msg.content.split("**Suggestions so Far:**").collect();
            content_parts.get(0).unwrap_or(&"").to_string()
        };

        let mut results_write = self.results.write().await;
        if let Some(message) = results_write.get_mut(g) {
            let new_content = if suggestions.is_empty() {
                format!("{}**Suggestions so Far:**\nNo suggestions yet", &base_content)
            } else {
                format!("{}**Suggestions so Far:**\n{}", &base_content, suggestions.join("\n"))
            };

            // Edit message with timeout protection
            let edit_result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                message.edit(context, EditMessage::new().content(new_content))
            ).await;

            match edit_result {
                Ok(Ok(_)) => {}, // Success
                Ok(Err(e)) => return Err(format!("Failed to edit message: {}", e)),
                Err(_) => return Err("Timeout editing message".to_string()),
            }
        }

        Ok(())
    }

    /// Updates the most recent poll announcement in the given guild with the latest polling
    /// numbers.
    async fn poll_votes(&self, context: Context, g: &GuildId) {
        let winners = self.winners(g).await.join("\n");

        if !self.results.read().await.contains_key(g) {
            return;
        }

        // Edit the results section in the new poll message to have the winning candidates
        let cts = self
            .results
            .read()
            .await
            .get(g)
            .unwrap()
            .content
            .split_inclusive("**Results so Far:**")
            .map(|s| s.to_owned())
            .next()
            .unwrap_or_default();

        // Acquire mutable access to the stored message, build the new content,
        // and edit it with timeout + proper error handling.
        let mut results_write = self.results.write().await;
        if let Some(message) = results_write.get_mut(g) {
            let new_content = format!("{}\n{}", cts, winners);

            match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                message.edit(&context, EditMessage::new().content(new_content))
            ).await {
                Ok(Ok(_)) => {
                    println!("Successfully updated vote results for guild {}", g);
                },
                Ok(Err(e)) => {
                    eprintln!("Discord API error updating vote results: {}", e);
                },
                Err(_) => {
                    eprintln!("Timeout updating vote results for guild {}", g);
                }
            }
        }
    }

    /// Checks whether the vote is currently in the suggestion period.
    async fn in_suggestion_period(&self, guild_id: &GuildId) -> bool {
        !self
            .upcoming_topics
            .get(guild_id)
            .unwrap()
            .read()
            .await
            .is_empty()
    }

    /// Checks whether the vote is currently in the voting period.
    /// If no votes are cast, it is not in the voting period.
    async fn in_vote_period(&self, guild_id: &GuildId) -> bool {
        !self.votes.get(guild_id).unwrap().read().await.is_empty()
    }

    // ===== INTERACTION RESPONSE HELPERS =====

    /// Send an immediate response to the user
    async fn send_response(&self, ctx: &Context, command: &CommandInteraction, content: &str) {
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content)
        );

        if let Err(why) = command.create_response(&ctx.http, response).await {
            eprintln!("Failed to respond to command '{}': {}", command.data.name, why);
        }
    }

    /// Send an ephemeral (private) response to the user
    async fn send_ephemeral_response(&self, ctx: &Context, command: &CommandInteraction, content: &str) {
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(content)
                .ephemeral(true)
        );

        if let Err(why) = command.create_response(&ctx.http, response).await {
            eprintln!("Failed to send ephemeral response: {}", why);
        }
    }

    /// Defer the response for long-running operations
    async fn defer_response(&self, ctx: &Context, command: &CommandInteraction, ephemeral: bool) -> bool {
        let response = if ephemeral {
            CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))
        } else {
            CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new())
        };

        match command.create_response(&ctx.http, response).await {
            Ok(()) => true,
            Err(why) => {
                eprintln!("Failed to defer response: {}", why);
                false
            }
        }
    }

    /// Send a follow-up message after deferring with timeout protection
    async fn send_followup(&self, ctx: &Context, command: &CommandInteraction, content: &str) {
        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            command.create_followup(ctx, CreateInteractionResponseFollowup::new().content(content))
        ).await {
            Ok(Ok(_)) => {
                println!("Successfully sent follow-up response to user: {}", command.user.id);
            },
            Ok(Err(why)) => {
                eprintln!("Discord API error in follow-up: {}", why);
            },
            Err(_) => {
                eprintln!("Follow-up response timeout - Discord API took too long");
            }
        }
    }

    /// Send a follow-up message with guaranteed delivery (fallback to error message)
    async fn send_followup_guaranteed(&self, ctx: &Context, command: &CommandInteraction, content: &str) {
        let fallback_msg = "⚠️ Operation completed but response delivery failed. Please check the announcements channel.";

        match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            command.create_followup(ctx, CreateInteractionResponseFollowup::new().content(content))
        ).await {
            Ok(Ok(_)) => {
                println!("Successfully sent follow-up response to user: {}", command.user.id);
                return;
            },
            Ok(Err(why)) => {
                eprintln!("Discord API error in follow-up, trying fallback: {}", why);
            },
            Err(_) => {
                eprintln!("Follow-up response timeout, trying fallback message");
            }
        }

        // Fallback attempt with error message
        if let Err(why) = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            command.create_followup(ctx, CreateInteractionResponseFollowup::new().content(fallback_msg))
        ).await {
            eprintln!("Critical: Failed to send any follow-up response: {:?}", why);
        }
    }

    // ===== COMMAND HANDLERS WITH PROPER RESPONSE PATTERNS =====

    async fn handle_prop_command(&self, ctx: &Context, command: &CommandInteraction) {
        let idea = match command.data.options.get(0)
            .map(|opt| &opt.value)
            .and_then(|val| val.as_str()) {
            Some(idea) if !idea.trim().is_empty() => idea.trim().to_string(),
            _ => {
                self.send_ephemeral_response(ctx, command, "❌ Please provide a valid proposal idea!").await;
                return;
            },
        };

        // Defer response since announcing and state updates might take time
        if !self.defer_response(ctx, command, false).await {
            eprintln!("Failed to defer response for /prop command from user: {}", command.user.id);
            return;
        }

        println!("Processing /prop command for user: {} with idea: {}", command.user.id, idea);

        // Execute with timeout protection
        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(12),
            self.slash_prop(ctx, command, idea.clone())
        ).await {
            Ok(result) => result,
            Err(_) => {
                eprintln!("Timeout processing /prop command for user: {}", command.user.id);
                format!("⏱️ Operation timed out, but your proposal '{}' may have been recorded. Please check the announcements channel.", idea)
            }
        };

        self.send_followup_guaranteed(ctx, command, &result).await;
        println!("Completed /prop command processing for user: {}", command.user.id);
    }

    async fn handle_vote_command(&self, ctx: &Context, command: &CommandInteraction) {
        let votes = command.data.options.get(0)
            .map(|opt| &opt.value)
            .and_then(|val| val.as_i64())
            .filter(|&v| v > 0 && v <= 10)
            .unwrap_or(0) as usize;

        let candidate_id = command.data.options.get(1)
            .map(|opt| &opt.value)
            .and_then(|val| val.as_i64())
            .filter(|&v| v >= 0)
            .unwrap_or(-1) as isize;

        if votes == 0 {
            self.send_ephemeral_response(ctx, command, "❌ Number of votes must be between 1 and 10!").await;
            return;
        }

        if candidate_id < 0 {
            self.send_ephemeral_response(ctx, command, "❌ Please provide a valid candidate ID (0 or higher)!").await;
            return;
        }

        let result = self.slash_vote(ctx, command, votes, candidate_id as usize).await;
        self.send_response(ctx, command, &result).await;
    }

    async fn handle_points_command(&self, ctx: &Context, command: &CommandInteraction) {
        let result = self.slash_points(ctx, command).await;
        self.send_ephemeral_response(ctx, command, &result).await; // Points are private
    }

    async fn handle_start_command(&self, ctx: &Context, command: &CommandInteraction) {
        let prompt = match command.data.options.get(0)
            .map(|opt| &opt.value)
            .and_then(|val| val.as_str()) {
            Some(prompt) if !prompt.trim().is_empty() => prompt.trim().to_string(),
            _ => {
                self.send_ephemeral_response(ctx, command, "❌ Please provide a valid election prompt!").await;
                return;
            },
        };

        // Defer response since starting an election might take time
        if !self.defer_response(ctx, command, false).await {
            eprintln!("Failed to defer response for /start command from user: {}", command.user.id);
            return;
        }

        println!("Processing /start command for user: {} with prompt: {}", command.user.id, prompt);

        // Execute with timeout protection - start command can be complex
        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.slash_start(ctx, command, prompt.clone())
        ).await {
            Ok(result) => result,
            Err(_) => {
                eprintln!("Timeout processing /start command for user: {}", command.user.id);
                format!("⏱️ Election start operation timed out. Please check the announcements channel and try again if needed.")
            }
        };

        self.send_followup_guaranteed(ctx, command, &result).await;
        println!("Completed /start command processing for user: {}", command.user.id);
    }

    async fn handle_stop_command(&self, ctx: &Context, command: &CommandInteraction) {
        // Defer response since stopping might take time to calculate results
        if !self.defer_response(ctx, command, false).await {
            eprintln!("Failed to defer response for /stop command from user: {}", command.user.id);
            return;
        }

        println!("Processing /stop command for user: {}", command.user.id);

        // Execute with timeout protection
        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.slash_stop(ctx, command)
        ).await {
            Ok(result) => result,
            Err(_) => {
                eprintln!("Timeout processing /stop command for user: {}", command.user.id);
                "⏱️ Election stop operation timed out. Please check the announcements channel for status.".to_string()
            }
        };

        self.send_followup_guaranteed(ctx, command, &result).await;
        println!("Completed /stop command processing for user: {}", command.user.id);
    }

    // ===== SLASH COMMAND HANDLERS =====

    async fn slash_prop(&self, ctx: &Context, command: &CommandInteraction, idea: String) -> String {
        let Some(guild_id) = command.guild_id else {
            return "❌ This command can only be used in a server!".to_string();
        };

        // Check rate limiting
        if self.check_rate_limit(guild_id, command.user.id).await {
            return "⏱️ Please wait 2 seconds between commands!".to_string();
        }

        // Check if the idea is too long
        if idea.len() > 100 {
            return "❌ Proposal ideas must be 100 characters or less!".to_string();
        }

        // Check if in voting period
        if self.in_vote_period(&guild_id).await {
            return "❌ Candidates cannot be proposed while the vote is ongoing!".to_string();
        }

        // Safe access to guild data
        let Some(topics_lock) = self.upcoming_topics.get(&guild_id) else {
            return "❌ Server not configured for voting. Contact an administrator.".to_string();
        };

        // Check for duplicates with proper error handling - scope the read lock
        let is_duplicate = {
            topics_lock.read().await.contains(&idea)
        };

        match is_duplicate {
            true => format!("❌ Your proposal '{}' already exists!", idea),
            false => {
                // Add the proposal with error handling
                println!("Attempting to store proposal '{}' for guild {}", idea, guild_id);
                topics_lock.write().await.push(idea.clone());
                println!("Successfully stored proposal '{}' for guild {}", idea, guild_id);

                // Update suggestions display (only if election is active)
                if let Err(e) = self.poll_suggestions_safe(ctx, &guild_id).await {
                    // Silent fail if no active election - this is normal for first proposals
                    eprintln!("No active election to update: {}", e);
                }

                // Announce in channel (non-blocking)
                if let Some(_) = announce!(ctx, guild_id, format!("🗳️ New candidate proposed: {}", idea)) {
                    // Announcement successful
                } else {
                    eprintln!("Failed to announce new proposal in guild {} - channel not found or no permissions", guild_id);
                }

                format!("✅ Proposal '{}' added successfully!", idea)
            }
        }
    }

    async fn slash_vote(&self, ctx: &Context, command: &CommandInteraction, votes: usize, candidate_id: usize) -> String {
        let Some(guild_id) = command.guild_id else {
            return "❌ This command can only be used in a server!".to_string();
        };

        // Check rate limiting
        if self.check_rate_limit(guild_id, command.user.id).await {
            return "⏱️ Please wait 2 seconds between commands!".to_string();
        }

        // Validate vote count
        if votes == 0 || votes > 10 {
            return "❌ Number of votes must be between 1 and 10!".to_string();
        }

        // Safe access to guild data
        let Some(votes_lock) = self.votes.get(&guild_id) else {
            return "❌ Server not configured for voting. Contact an administrator.".to_string();
        };

        let Some(points_lock) = self.points.get(&guild_id) else {
            return "❌ Server not configured for voting. Contact an administrator.".to_string();
        };

        // Convert user's 1-based candidate ID to 0-based internal index
        if candidate_id == 0 {
            return "❌ Candidate IDs start from 1. Use `/vote <votes> <candidate_id>` where candidate_id ≥ 1".to_string();
        }
        let internal_candidate_id = candidate_id - 1;

        // Check if candidate exists
        let votes_read = votes_lock.read().await;
        if !votes_read.contains_key(&internal_candidate_id) {
            return format!("❌ Candidate #{} does not exist!", candidate_id);
        }
        drop(votes_read);

        // Initialize user points if needed
        if !points_lock.read().await.contains_key(&command.user.id) {
            points_lock.write().await.insert(command.user.id, AtomicUsize::new(STARTING_POINTS));
        }

        let req_points = votes.pow(2);
        let mut can_spend = points_lock.read().await
            .get(&command.user.id).unwrap().load(Ordering::Relaxed);

        // Check for existing votes and calculate refund
        let votes_read = votes_lock.read().await;
        if let Some(candidate) = votes_read.get(&internal_candidate_id) {
            if let Some(existing_votes) = candidate.2.get(&command.user.id) {
                can_spend += existing_votes.load(Ordering::Relaxed).pow(2);
            }
        }
        drop(votes_read);

        if can_spend < req_points {
            return format!("❌ Insufficient points! {} votes cost {} points, but you can only spend {}.",
                votes, req_points, can_spend);
        }

        // Process the vote with proper error handling
        let mut votes_map = votes_lock.write().await;
        if let Some(candidate_entry) = votes_map.get_mut(&internal_candidate_id) {
            // Handle existing votes refund
            if let Some(existing_votes) = candidate_entry.2.get(&command.user.id) {
                let prev_votes = existing_votes.swap(votes, Ordering::Relaxed);
                candidate_entry.1.fetch_sub(prev_votes, Ordering::Relaxed);
                points_lock.read().await.get(&command.user.id).unwrap()
                    .fetch_add(prev_votes.pow(2), Ordering::Relaxed);
            } else {
                candidate_entry.2.insert(command.user.id, AtomicUsize::new(votes));
            }

            // Add new votes and charge points
            candidate_entry.1.fetch_add(votes, Ordering::Relaxed);
            points_lock.read().await.get(&command.user.id).unwrap()
                .fetch_sub(req_points, Ordering::Relaxed);
        } else {
            return format!("❌ Candidate #{} no longer exists!", candidate_id);
        }
        drop(votes_map);

        // Update results (non-blocking)
        self.poll_votes(ctx.clone(), &guild_id).await;

        let remaining = points_lock.read().await
            .get(&command.user.id).unwrap().load(Ordering::Relaxed);

        format!("✅ Cast {} votes for candidate #{}! Points remaining: {}", votes, candidate_id, remaining)
    }

    async fn slash_points(&self, _ctx: &Context, command: &CommandInteraction) -> String {
        let Some(guild_id) = command.guild_id else {
            return "❌ This command can only be used in a server!".to_string();
        };

        // Safe access to guild data
        let Some(points_lock) = self.points.get(&guild_id) else {
            return "❌ Server not configured for voting. Contact an administrator.".to_string();
        };

        let points_left = points_lock.read().await
            .get(&command.user.id)
            .map(|a| a.load(Ordering::Relaxed))
            .unwrap_or(STARTING_POINTS);

        format!("🗳️ You have **{}** points left (out of {}) to spend in this election.",
            points_left, STARTING_POINTS)
    }

    async fn slash_start(&self, ctx: &Context, command: &CommandInteraction, prompt: String) -> String {
        let Some(guild_id) = command.guild_id else {
            return "❌ This command can only be used in a server!".to_string();
        };

        // Check if guild exists in cache
        if ctx.cache.guild(guild_id).is_none() {
            return "❌ Unable to access server information. Please try again.".to_string();
        };

        // Check admin permissions with timeout protection
        let has_permission = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            async { check_admin_permission!(ctx, guild_id, command.user) }
        ).await.unwrap_or(false);

        if !has_permission {
            return format!(
                "❌ You need one of the following to start an election:\n• Server Owner\n• Administrator permission\n• '{}' role",
                BOT_ROLE
            );
        }

        println!("User {} has permission to start election in guild {}", command.user.id, guild_id);

        // Stop any ongoing election first with timeout protection
        let stop_result = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            self.slash_stop_internal(ctx, guild_id)
        ).await;

        if stop_result.is_err() {
            eprintln!("Timeout stopping previous election in guild {}", guild_id);
        }

        // Find announcement channel with error handling
        let channel_id = ctx.cache.guild(guild_id)
            .and_then(|guild| guild.channels.iter().find(|(_, ch)| ch.name == BOT_CHANNEL).map(|(id, _)| *id));

        let Some(channel_id) = channel_id else {
            return format!("❌ Announcement channel '{}' not found. Please create it first.", BOT_CHANNEL);
        };

        // Create election announcement with timeout protection
        let announcement_content = format!(
            "@everyone 🗳️ **An election has started:** {}\n\nSuggest candidates with `/prop <idea>`\n\n⏰ Time remaining: {}h\n\n**Suggestions so Far:**\nNo suggestions yet",
            prompt, SUGG_INTERVAL
        );

        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            channel_id.say(ctx, announcement_content)
        ).await {
            Ok(Ok(message)) => {
                self.results.write().await.insert(guild_id, message);
                println!("Successfully created election announcement in guild {}", guild_id);
                format!("✅ Election started: '{}'", prompt)
            },
            Ok(Err(why)) => {
                eprintln!("Failed to create election announcement: {}", why);
                format!("⚠️ Election started but failed to post announcement: '{}'. Please check channel permissions.", prompt)
            },
            Err(_) => {
                eprintln!("Timeout creating election announcement in guild {}", guild_id);
                format!("⚠️ Election started but announcement timed out: '{}'. Please check the announcements channel.", prompt)
            }
        }
    }

    async fn slash_stop(&self, ctx: &Context, command: &CommandInteraction) -> String {
        if let Some(guild_id) = command.guild_id {
            if let Some(_guild) = ctx.cache.guild(guild_id) {
                // Check admin permissions (role, administrator, or owner)
                if !check_admin_permission!(ctx, guild_id, command.user) {
                    return format!(
                        "❌ You need one of the following to stop an election:\n• Server Owner\n• Administrator permission\n• '{}' role",
                        BOT_ROLE
                    );
                }
            }

            self.slash_stop_internal(ctx, guild_id).await
        } else {
            "❌ This command can only be used in a server!".to_string()
        }
    }

    async fn slash_stop_internal(&self, ctx: &Context, guild_id: GuildId) -> String {
        // Check if in suggestion period
        if !self.upcoming_topics.get(&guild_id).unwrap().read().await.is_empty() {
            // Move from suggestions to voting
            let all_candidates: Vec<String> = self.upcoming_topics.get(&guild_id).unwrap().read().await.iter().cloned().collect();
            
            let mut candidates_str = String::new();
            for (i, name) in all_candidates.iter().enumerate() {
                self.votes.get(&guild_id).unwrap().write().await
                    .insert(i, (name.clone(), AtomicUsize::new(0), HashMap::new()));
                candidates_str = format!("{}#{}: {}\n", candidates_str, i + 1, name);
            }

            // Clear suggestions
            self.upcoming_topics.get(&guild_id).unwrap().write().await.clear();

            // Post voting message
            let channel_id = ctx.cache.guild(guild_id)
                .and_then(|guild| guild.channels.iter().find(|(_, ch)| ch.name == BOT_CHANNEL).map(|(id, _)| *id));

            if let Some(channel_id) = channel_id {
                if let Ok(message) = channel_id.say(ctx, format!(
                    "@everyone 🗳️ **Candidates selected:**\n{}\nVote with `/vote <votes> <candidate_number>`\n\n**Results so Far:**\nNo votes cast yet!",
                    candidates_str
                )).await {
                    self.results.write().await.insert(guild_id, message);
                }
            }

            "✅ Moved to voting phase!".to_string()
        } else if !self.votes.get(&guild_id).unwrap().read().await.is_empty() {
            // End voting and show results
            let winners = self.winners(&guild_id).await.join("\n");
            
            let channel_id = ctx.cache.guild(guild_id)
                .and_then(|guild| guild.channels.iter().find(|(_, ch)| ch.name == BOT_CHANNEL).map(|(id, _)| *id));

            if let Some(channel_id) = channel_id {
                let _ = channel_id.say(ctx, format!(
                    "@everyone 🏆 **The election is over!**\n\n**Winners:**\n{}",
                    winners
                )).await;
            }

            // Reset state
            self.votes.get(&guild_id).unwrap().write().await.clear();
            for (_user, points) in self.points.get(&guild_id).unwrap().read().await.iter() {
                points.swap(STARTING_POINTS, Ordering::Relaxed);
            }

            "✅ Election completed and results announced!".to_string()
        } else {
            "❌ No active election to stop!".to_string()
        }
    }
}

#[tokio::main]
async fn main() {
    // Above var can be in proc variables or .env
    dotenv().ok();

    // The discord API key is necessary for the bot to function
    let token =
        env::var(SECRET_KEY).expect(formatcp!("missing discord API secret in {}", SECRET_KEY));

    // Parse approved servers from environment variable
    let approved_servers_str = env::var(APPROVED_SERVERS_KEY)
        .expect(formatcp!("missing {} in environment variables", APPROVED_SERVERS_KEY));
    
    let approved_servers: Vec<GuildId> = approved_servers_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| GuildId::new(s.parse::<u64>().expect(&format!("Invalid server ID: {}", s))))
        .collect();

    println!("Bot configured for {} server(s): {:?}", approved_servers.len(), approved_servers);

    let handler = <Handler as Default>::default().register_servers(approved_servers);

    // Set gateway intents for slash commands and guild operations
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_MESSAGES;

    // Run the bot
    Client::builder(token, intents)
        .event_handler(handler)
        .await
        .expect("failed to create client")
        .start()
        .await
        .unwrap();
}
