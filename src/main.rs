#[macro_use]
extern crate const_format;

#[macro_use]
extern crate async_recursion;

use dotenv::dotenv;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::model::{
    channel::Message,
    id::{GuildId, UserId},
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::num::ParseIntError;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time;

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

/// The bot can be summoned through commands prefixed by:
const BOT_PREFIX: &str = "!";

/// The name of the command used to add a proposal
const PROP_CMD: &str = "prop";

/// The name of the command used to cast a vote
const VOTE_CMD: &str = "vote";

/// The name of the command used to check the user's balance
const POINTS_CMD: &str = "points";

/// The name of the command used to start a new voting process
const START_CMD: &str = "start";

/// The name of the command used to stop the current voting step
const STOP_CMD: &str = "stop";

/// The servers that the bot has been pre-approved for (whitelist only):
/// - No Filter Podcast
const APPROVED_SERVERS: [GuildId; 1] = [GuildId(936062001820622888)];

// Make an announcement, granted that the user is a user with those privileges
macro_rules! announce {
    ($context:expr,$msg:ident,$cts:expr) => {
        if let Some(ann_fut) = $msg.guild($context).await {
            if let Some(ann) = ann_fut.channel_id_from_name($context, BOT_CHANNEL).await {
                ann.say($context, $cts).await.ok()
            } else {
                None
            }
        } else {
            $msg.author
                .dm($context, |m| {
                    m.content(format!("Failed to {}: guild could not be found!", $cts))
                })
                .await
                .expect("discord API error");

            None
        }
    };
}

// Prevents a user from performing this action
macro_rules! role_gate {
    ($context:expr,$guild:ident,$msg:ident,$action:expr) => {
        // Ensure that the user executing the command has the BOT_ROLE
        if let Some(role) = $guild.role_by_name(BOT_ROLE) {
            if !$msg
                .author
                .has_role($context, $guild.id, role)
                .await
                .unwrap_or_default()
            {
                $msg.author
                    .dm($context, |m| {
                        m.content(format!(
                            "You must have the {} role to {}!",
                            BOT_ROLE, $action
                        ))
                    })
                    .await
                    .expect("discord API error");
            }
        }
    };
}

/// Possible commands for the quadratic voting bot:
/// !prop <topic>: Adds a topic to the upcoming election
/// !vote <n>, <topic>: Cast n votes for the selected topic
/// !points: Get the sender's remaining points in the election
/// !start <prompt>: Starts a new vote (can only be called by users with `vote` role)
/// !stop: Stops the segment of the voting process (can only be called by users with `vote` role)
#[derive(Default)]
struct Handler {
    // Suggested topics for the upcoming election
    upcoming_topics: HashMap<GuildId, Arc<RwLock<HashSet<String>>>>,

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
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, context: Context, msg: Message) {
        // Ignore all messages without the above prefix
        if msg.content.len() == 0 || msg.content.get(..1) != Some(BOT_PREFIX) {
            return;
        }

        // The first argument should be the name of the command
        let (cmd, sargs) = msg.content[1..]
            .split_once(' ')
            .unwrap_or((&msg.content[1..], ""));

        // Arguments must be manipulated later
        let args = sargs.to_owned();

        // See above explanation of different commands
        match cmd {
            PROP_CMD => self.prop(context, &msg, args).await,
            VOTE_CMD => self.vote(context, &msg, args).await,
            POINTS_CMD => self.points(context, &msg, args).await,
            START_CMD => self.start(context, &msg, args).await,
            STOP_CMD => self.stop(&context, &msg).await,
            _ => (),
        }
    }
}

impl Handler {
    /// Creates buckets for all of the pre-specified servers the bot belongs to.
    fn register_servers(mut self) -> Self {
        for g in APPROVED_SERVERS {
            self.upcoming_topics
                .insert(g, Arc::new(RwLock::new(HashSet::new())));
            self.points.insert(g, Arc::new(RwLock::new(HashMap::new())));
            self.votes.insert(g, Arc::new(RwLock::new(HashMap::new())));
        }

        self
    }

    /// !prop <topic>: Adds a topic to the upcoming election
    async fn prop(&self, context: Context, msg: &Message, args: String) {
        // Users cannot propose while voting
        if self.in_vote_period(msg).await {
            msg.author
                .dm(&context, |m| {
                    m.content("Candidates cannot be proposed while the vote is ongoing!")
                })
                .await
                .expect("discord API error");

            return;
        }

        if let Some(g) = msg.guild(&context).await {
            if self
                .upcoming_topics
                .get(&g.id)
                .unwrap()
                .read()
                .await
                .contains(args.as_str())
            {
                // Alert the user if their proposal already exists
                msg.author
                    .dm(&context, |m| {
                        m.content(format!(
                            "Your proposal {} already exists! Not adding.",
                            args
                        ))
                    })
                    .await
                    .expect("discord API error");
            }

            // Register the new candidate
            self.upcoming_topics
                .get(&g.id)
                .unwrap()
                .write()
                .await
                .insert(args.trim_end().to_owned());
            self.poll_suggestions(&context, &g.id).await;
            announce!(&context, msg, format!("New candidate proposed: {}", args));
        }
    }

    /// !vote <n>, <topic>: Cast n votes for the selected topic
    async fn vote(&self, context: Context, msg: &Message, args: String) {
        // Extract the n and topic ID from the arguments
        if let Ok(nargs) = args
            .split(" ")
            .map(|s| s.parse())
            .collect::<Result<Vec<usize>, ParseIntError>>()
        {
            let (n, candidate_id) = (nargs[0], nargs[1]);

            if let Some(guild) = msg.guild(&context).await {
                let g = &guild.id;

                // Reference to an atomic uint storing the user's vote count for the candidate
                if let Some(candidate_votes) =
                    self.votes.get(g).unwrap().read().await.get(&candidate_id)
                {
                    // Each user starts with STARTING_POINTS points
                    if !self
                        .points
                        .get(g)
                        .unwrap()
                        .read()
                        .await
                        .contains_key(&msg.author.id)
                    {
                        self.points
                            .get(g)
                            .unwrap()
                            .write()
                            .await
                            .insert(msg.author.id, AtomicUsize::new(STARTING_POINTS));
                    }

                    if let Some(existing_votes) = candidate_votes.2.get(&msg.author.id) {
                        // The user will be refunded the points they've already spent on this candidate.
                        // Calculate conversions from votes to points accordingly
                        let can_spend = self
                            .points
                            .get(g)
                            .unwrap()
                            .read()
                            .await
                            .get(&msg.author.id)
                            .unwrap()
                            .load(Ordering::Relaxed)
                            + existing_votes.load(Ordering::Relaxed).pow(2);
                        let req_points = n.pow(2);

                        // The user cannot continue if they lack the requisite points
                        if can_spend < req_points {
                            msg.author.dm(&context, |m| m.content(format!("Insufficient points to cast votes: {} votes cost {} points, but you can only spend {}.", n, req_points, can_spend))).await.expect("discord API error");

                            return;
                        }

                        // Refund the user's votes previously delegated to the candidate
                        let prev_votes = candidate_votes
                            .2
                            .get(&msg.author.id)
                            .unwrap()
                            .swap(n, Ordering::Relaxed);

                        // Remove old votes for the candidate
                        candidate_votes.1.fetch_sub(prev_votes, Ordering::Relaxed);

                        // Refund the user's points
                        self.points
                            .get(g)
                            .unwrap()
                            .read()
                            .await
                            .get(&msg.author.id)
                            .unwrap()
                            .fetch_add(prev_votes, Ordering::Relaxed);
                    }

                    // Subtract the user's spent points and allocate the votes
                    candidate_votes.1.fetch_add(n, Ordering::Relaxed);
                    self.points
                        .get(g)
                        .unwrap()
                        .read()
                        .await
                        .get(&msg.author.id)
                        .unwrap()
                        .fetch_sub(n, Ordering::Relaxed);

                    // Recalculate the vote balance announcement
                    self.poll_votes(context, g).await;
                }
            }
        } else {
            msg.author
                .dm(&context, |m| {
                    m.content("Missing parameters. Usage: ```\n!vote <n>, <topic id>\n```")
                })
                .await
                .expect("discord API error");
        }
    }

    /// !points: Get the sender's remaining points in the election
    async fn points(&self, context: Context, msg: &Message, _args: String) {
        if let Some(g) = msg.guild(&context).await {
            let points_left = self
                .points
                .get(&g.id)
                .unwrap()
                .read()
                .await
                .get(&msg.author.id)
                .map(|a| a.load(Ordering::Relaxed))
                .unwrap_or(STARTING_POINTS);
            // Send the user the number of points they have left
            msg.author
                .dm(&context, |m| {
                    m.content(format!(
                        "You have {} points left (out of {}) to spend in this election.",
                        points_left, STARTING_POINTS,
                    ))
                })
                .await
                .expect("discord API error");
        }
    }

    /// !start: Starts an election
    async fn start(&self, context: Context, msg: &Message, args: String) {
        if let Some(g) = msg.guild(&context).await {
            role_gate!(&context, g, msg, "start an election");

            // Stop any currently ongoing election
            self.stop(&context, msg).await;

            // Announce the vote
            if let Some(live_props) = announce!(&context, msg, format!("@everyone An election has started: {}\nSuggest candidates with !prop <idea>\n\nTime remaining: {}h\n**Suggestions so Far:**\nNo suggestions", args, VOTE_INTERVAL)) {
                self.results.write().await.insert(g.id, live_props);
            }

            // Stop the suggestion election after the voting interval
            // Count up the suggestions
            time::sleep(Duration::from_secs(SUGG_INTERVAL * 60 * 60)).await;

            // If the user hasn't already cleared the suggestions, do it automatically
            if self.in_suggestion_period(msg).await {
                self.stop(&context, msg).await;
            }
        }
    }

    #[async_recursion]
    /// !stop: Stops the currently running step of the election
    async fn stop(&self, context: &Context, msg: &Message) {
        // Clear out the proposals and list them in an announcement
        if self.in_suggestion_period(msg).await {
            if let Some(g) = msg.guild(context).await {
                role_gate!(&context, g, msg, "stop an election");

                let all_candidates = self.upcoming_topics.get(&g.id).unwrap().read().await;

                // Buffer for the string representation of these candidates
                let mut candidates_str = String::new();

                // Clear out all candidates and set their vote counts to zero
                for (i, name) in all_candidates.iter().enumerate() {
                    self.votes
                        .get(&g.id)
                        .unwrap()
                        .write()
                        .await
                        .insert(i, (name.clone(), AtomicUsize::new(0), HashMap::new()));

                    // Display the candidates with their indices
                    candidates_str = format!("{}#{}: {}\n", candidates_str, i, name);
                }

                // Remove all candidates
                self.upcoming_topics
                    .get(&g.id)
                    .unwrap()
                    .write()
                    .await
                    .clear();

                // Store the announcement message for later reporting
                if let Some(live_results) = announce!(context, msg, format!("@everyone Candidates have been selected:\n{}\nVote with !vote <n votes>, <candidate number>\n**Results so Far:**\nNo votes cast!", candidates_str)) {
                    self.results.write().await.insert(g.id, live_results);
                }

                // Allow user to cast votes, and then automatically stop the count after the
                // interval
                time::sleep(Duration::from_secs(VOTE_INTERVAL * 60 * 60)).await;
                if self.in_vote_period(msg).await {
                    self.stop(context, msg).await;
                }
            }
        // Calculate the results of the election
        } else if self.in_vote_period(msg).await {
            if let Some(g) = msg.guild(context).await {
                let winners = self.winners(&g.id).await.join("\n");

                // Display the first n winning candidates, their names, and their votes
                announce!(
                    context,
                    msg,
                    format!(
                        "@everyone The election is over. The winners are:\n{}",
                        winners,
                    )
                );

                // Clear the votes
                self.votes.get(&g.id).unwrap().write().await.clear();

                // Reset point counts
                for (_user, points) in self.points.get(&g.id).unwrap().read().await.iter() {
                    points.swap(STARTING_POINTS, Ordering::Relaxed);
                }
            }
        }
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
            .enumerate()
            .map(|(i, w)| format!("#{} {}: {}", i, w.0, w.1))
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
            .edit(context, |m| {
                m.content(format!("{}\n{}", cts, suggestions.join("\n")))
            })
            .await
            .expect("discord API error");
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

        self.results
            .write()
            .await
            .get_mut(g)
            .unwrap()
            .edit(context, |m| m.content(format!("{}\n{}", cts, winners)))
            .await
            .expect("discord API error");
    }

    /// Checks whether the vote is currently in the suggestion period.
    async fn in_suggestion_period(&self, msg: &Message) -> bool {
        if let Some(g) = msg.guild_id {
            !self
                .upcoming_topics
                .get(&g)
                .unwrap()
                .read()
                .await
                .is_empty()
        } else {
            true
        }
    }

    /// Checks whether the vote is currently in the suggestion period.
    /// If no votes are cast, it is not in the voting period.
    async fn in_vote_period(&self, msg: &Message) -> bool {
        if let Some(g) = msg.guild_id {
            !self.votes.get(&g).unwrap().read().await.is_empty()
        } else {
            true
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

    let handler = <Handler as Default>::default().register_servers();

    // Run the bot
    Client::builder(token)
        .event_handler(handler)
        .await
        .expect("failed to create client")
        .start()
        .await
        .unwrap();
}
