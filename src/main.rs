use dotenv::dotenv;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::framework::standard::{
    macros::{command, group},
    CommandResult, StandardFramework,
};
use serenity::model::{
    channel::Message,
    guild::Guild,
    id::{GuildId, UserId},
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
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
const SUGG_INTERVAL: usize = 48;

/// The number of hours that a vote should last
const VOTE_INTERVAL: usize = 24;

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

// Make an announcement, granted that the user is a user with those privileges
macro_rules! announce {
    ($msg:ident,$cts:expr) => {
        if let Some(ann) = msg
            .guild()
            .await
            .map(|g| g.channel_id_from_name(BOT_CHANNEL))
            .await
        {
            ann.say().await
        } else {
            msg.author
                .dm(|m| m.content("Failed to start election: guild could not be found!"))
                .await;

            None
        }
    };
}

// Prevents a user from performing this action
macro_rules! role_gate {
    ($msg:ident,$action:expr) => {
        // Ensure that the user executing the command has the BOT_ROLE
        if !msg
            .author
            .has_role(msg.guild.await.role_by_name(BOT_ROLE))
            .await
            .unwrap_or_default()
        {
            msg.author.dm(|m| {
                m.content(format!(
                    "You must have the {} role to {}!",
                    BOT_ROLE, action
                ))
            });
        }
    };
}

/// Possible commands for the quadratic voting bot:
/// !prop <topic>: Adds a topic to the upcoming election
/// !vote <n>, <topic>: Cast n votes for the selected topic
/// !points: Get the sender's remaining points in the election
/// !start <prompt>: Starts a new vote (can only be called by users with `vote` role)
/// !stop: Stops the segment of the voting process (can only be called by users with `vote` role)
struct Handler {
    // Suggested topics for the upcoming election
    upcoming_topics: HashMap<GuildId, RwLock<HashSet<String>>>,

    // Users cannot have less than 0 points, but they may have different
    // balances per-guild
    points: HashMap<GuildId, HashMap<UserId, AtomicUsize>>,

    // The bot automatically updates results of the election as it progresses
    results: HashMap<GuildId, Cell<Option<Message>>>,

    // Total votes per idea, and votes cast per idea per user
    votes: HashMap<
        GuildId,
        RwLock<HashMap<usize, (String, AtomicUsize, HashMap<UserId, AtomicUsize>)>>,
    >,
}

/// Votes can be stopped by terminating the current step and continuing to the
/// next, or by terminating the vote altogether.
enum StopKind {
    /// Forcibly continue to the next step of the vote
    SOFT,

    /// Stop the vote entirely
    HARD,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, context: Context, msg: Message) {
        // Ignore all messages without the above prefix
        if msg.content.length() == 0 || msg.content[0] != BOT_PREFIX {
            return;
        }

        // The first argument should be the name of the command
        let (cmd, args) = msg.contents.split_once(' ');

        // See above explanation of different commands
        match msg.contents {
            PROP_CMD => self.prop(context, msg, args).await,
            VOTE_CMD => self.vote(context, msg, args).await,
            POINTS_CMD => self.points(context, msg, args).await,
            START_CMD => self.start(context, msg, args).await,
            STOP_CMD => self.stop(context, msg, args, StopKind::SOFT).await,
        }
    }

    /// !prop <topic>: Adds a topic to the upcoming election
    async fn prop(&self, context: Context, msg: Message, args: String) {
        // Users cannot propose while voting
        if self.in_vote_period(msg).await {
            msg.author
                .dm(|m| m.content("Candidates cannot be proposed while the vote is ongoing!"));

            return;
        }

        if let Some(g) = msg.guild().await {
            if self.upcoming_topics.get(g).read().contains(msg.contents) {
                // Alert the user if their proposal already exists
                msg.author
                    .dm(|m| {
                        m.content(format!(
                            "Your proposal {} already exists! Not adding.",
                            msg.contents
                        ))
                    })
                    .await;
            }

            // Register the new candidate
            self.upcoming_topics.get(g).write().add(msg.contents);
            announce!(msg, format!("New candidate proposed: {}", msg.contents));
        }
    }

    /// !vote <n>, <topic>: Cast n votes for the selected topic
    async fn vote(&self, context: Context, msg: Message, args: String) {
        // Extract the n and topic ID from the arguments
        if let Some((n, candidate_id)) = args.split().map(|s| s.parse()) {
            if let Some(g) = msg.guild().await {
                // Reference to an atomic uint storing the user's vote count for the candidate
                let candidate_votes = self.votes.get(g).read().get(candidate_id);

                // The user will be refunded the points they've already spent on this candidate.
                // Calculate conversions from votes to points accordingly
                let can_spend = self.points.get(g).get(msg.author).load(Ordering::Relaxed)
                    + candidate_votes
                        .2
                        .get(msg.author.id)
                        .load(Ordering::Relaxed)
                        .pow(2);
                let req_points = n.pow(2);

                // The user cannot continue if they lack the requisite points
                if can_spend < req_points {
                    msg.author.dm(|m| m.content(format!("Insufficient points to cast votes: {} votes cost {} points, but you can only spend {}.", n, req_points, can_spend))).await;

                    return;
                }

                // Refund the user's votes previously delegated to the candidate
                let prev_votes = candidate_votes
                    .2
                    .get(msg.author.id)
                    .swap(n, Ordering::Relaxed);

                // Remove old votes for the candidate, and calculate the new count
                candidate_votes.1.fetch_sub(prev_votes, Ordering::Relaxed);
                candidate_votes.1.fetch_add(n, Ordering::Relaxed);

                // Refund the user's votes for the selected topic
                self.points
                    .get(g)
                    .get(msg.author)
                    .fetch_add(prev_votes, Ordering::Relaxed);
                self.points
                    .get(g)
                    .get(msg.author)
                    .fetch_sub(n, Ordering::Relaxed);

                // Recalculate the vote balance announcement
            }
        } else {
            msg.author
                .dm(|m| m.content("Missing parameters. Usage: ```\n!vote <n>, <topic id>\n```"))
                .await;
        }
    }

    /// !points: Get the sender's remaining points in the election
    async fn points(&self, context: Context, msg: Message, args: String) {
        if let Some(g) = msg.guild().await {
            // Send the user the number of points they have left
            msg.author
                .dm(|m| {
                    m.content(format!(
                        "You have {} points left (out of {}) to spend in this election.",
                        self.points.get(g).get(msg.author.id),
                        STARTING_POINTS,
                    ))
                })
                .await;
        }
    }

    /// !start: Starts an election
    async fn start(&self, context: Context, msg: Message, args: String) {
        role_gate!(msg, "start an election");

        // Stop any currently ongoing election
        self.stop(context, msg, args, StopKind::HARD).await;

        // Announce the vote
        announce!(msg, format("@everyone An election has started: {}\nSuggest candidates with !prop <idea>\nTime remaining: {}h", msg.contents, VOTE_INTERVAL));

        // Stop the suggestion election after the voting interval
        // Count up the suggestions
        time::sleep(Duration::from_secs(SUGG_INTERVAL * 60 * 60)).await;

        // If the user hasn't already cleared the suggestions, do it automatically
        if self.in_suggestion_period(msg) {
            self.stop(context, msg, args, StopKind::SOFT).await;
        }
    }

    /// !stop: Stops the currently running step of the election
    async fn stop(&self, context: Context, msg: Message, args: String, kind: StopKind) {
        role_gate!(msg, "stop an election");

        // Clear out the proposals and list them in an announcement
        if self.in_suggestion_period(msg).await {
            if let Some(g) = msg.guild().await {
                // Display the candidates with their indices
                let candidates_str = self
                    .upcoming_topics
                    .get(g)
                    .read()
                    .iter()
                    .enumerate()
                    .fold(String::new(), |i, a, b| a + format!("#{}: {}\n", i + 1, b));

                // Clear out all candidates and set their vote counts to zero
                for (i, name) in self.upcoming_topics.get(g).read().keys().enumerate() {
                    self.votes
                        .get(g)
                        .write()
                        .insert(name, i, (0, HashMap::new()));
                }

                // Remove all candidates
                self.upcoming_topics.get(g).write().clear();

                // Store the announcement message for later reporting
                let live_results = announce!(msg, format!("@everyone Candidates have been selected:\n{}\nVote with !vote <n votes>, <candidate number>", candidates_str));
                self.results.get(g).replace(live_results);

                // Allow user to cast votes, and then automatically stop the count after the
                // interval
                time::sleep(Duration::from_secs(VOTE_INTERVAL * 60 * 60)).await;
                if self.in_vote_period(msg).await {
                    self.stop(context, msg, args, kind);
                }
            }
        // Calculate the results of the election
        } else if self.in_vote_period(msg).await {
            if let Some(g) = msg.guild().await {
                // Sort the candidates by their number of vote
                let winners = self
                    .votes
                    .get(g)
                    .read()
                    .values()
                    .collect::<Vec<(String, AtomicUsize, HashMap<UserId, AtomicUsize>)>>()
                    .sort_by(|b, a| a.1 .1.partial_cmp(b.1 .1).unwrap());

                // Display the first n winning candidates, their names, and their votes
                let winners_str = winners
                    .enumerate()
                    .map(|i, w| format!("#{} {}: {}\n", i, w.0, w.1))
                    .take(CONVENIENT_WINNERS)
                    .join("\n");

                announce!(
                    msg,
                    format!(
                        "@everyone The election is over. The winners are:\n{}",
                        winners_str
                    )
                );

                // Clear the votes
                self.votes.get(g).write().clear();

                // Reset point counts
                for k in self.points.get(g).keys() {
                    self.points
                        .get(g)
                        .get(k)
                        .swap(STARTING_POINTS, Ordering::Relaxed);
                }
            }
        }
    }

    /// Updates the most recent poll announcement in the given guild with the latest polling
    /// numbers.
    async fn poll_votes(&self, g: Guild) {
        // TODO: Implement live reporting

        // If no channel exists, the announcement cannot be updated
        if let Some(chann) = g.channel_id_from_name(BOT_CHANNEL).await {}
    }

    /// Checks whether the vote is currently in the suggestion period.
    async fn in_suggestion_period(&self, msg: Message) -> bool {
        msg.guild_id
            .map(|g| !self.upcoming_topics.get(g).read().is_empty())
            .unwrap_or_default()
    }

    /// Checks whether the vote is currently in the suggestion period.
    /// If no votes are cast, it is not in the voting period.
    async fn in_vote_period(&self, msg: Message) -> bool {
        msg.guild_id
            .map(|g| !self.votes.get(g).is_empty())
            .unwrap_or_default()
    }
}

#[tokio::main]
async fn main() {
    // Above var can be in proc variables or .env
    dotenv().ok();

    // The discord API key is necessary for the bot to function
    let token = env::var().expect(format!("missing discord API secret in {}", SECRET_KEY));

    // Run the bot
    Client::builder(token)
        .event_handler(Handler)
        .await
        .expect("failed to create client")
        .start()
        .await
        .unwrap();
}
