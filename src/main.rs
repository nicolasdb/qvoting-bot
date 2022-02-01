use dotenv::dotenv;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::framework::standard::{
    macros::{command, group},
    CommandResult, StandardFramework,
};
use serenity::model::{
    channel::Message,
    id::{GuildId, UserId},
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::atomic::AtomicUsize;

/// Where the discord api key should be stored in the process or .env environment
/// variables
const SECRET_KEY: &str = "DISCORD_SECRET";

/// Role name identifying users with the ability to start and stop elections.
const BOT_ROLE: &str = "voting";

/// Everyone starts out with 100 points, and they reset on the below interval:
const STARTING_POINTS: usize = 100;

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
/// !start: Starts a new vote (can only be called by users with `vote` role)
/// !stop: Stops the segment of the voting process (can only be called by users with `vote` role)
struct Handler {
    // Suggested topics for the upcoming election
    upcoming_topics: HashMap<GuildId, RwLock<HashSet<String>>>,

    // Users cannot have less than 0 points, but they may have different
    // balances per-guild
    points: HashMap<GuildId, HashMap<UserId, AtomicUsize>>,

    // Total votes per idea, and votes cast per idea per user
    votes: HashMap<GuildId, HashMap<String, (AtomicUsize, HashMap<UserId, AtomicUsize>)>>,
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
    async fn prop(&self, context: Context, msg: Message, args: String) {}

    /// !vote <n>, <topic>: Cast n votes for the selected topic
    async fn vote(&self, context: Context, msg: Message, args: String) {}

    /// !points: Get the sender's remaining points in the election
    async fn points(&self, context: Context, msg: Message, args: String) {}

    /// !start: Starts an election
    async fn start(&self, context: Context, msg: Message, args: String) {
        role_gate!(msg, "start an election");

        // Stop any currently ongoing election
        self.stop(context, msg, args, StopKind::HARD).await;

        // Stop the election after the voting interval
    }

    /// !stop: Stops the currently running step of the election
    async fn stop(&self, context: Context, msg: Message, args: String, kind: ) {
        role_gate!(msg, "stop an election");
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
