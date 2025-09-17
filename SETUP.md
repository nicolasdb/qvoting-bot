# 🛠️ Setup Guide

Quick setup guide for the Quadratic Voting Discord Bot.

## 🎮 Discord Bot Setup

### Create Discord Application
1. Go to [Discord Developer Portal](https://discord.com/developers/applications)
2. Click "New Application" and give it a name
3. Go to "Bot" section
4. **Reset & Copy the bot token** (you'll need paste this in .env)

### 🔑 Enable Privileged Gateway Intents
**⚠️ CRITICAL STEP - Bot will crash without these!**
1. Scroll down to **"Privileged Gateway Intents"**
2. **Enable these intents:**
   - ☑️ **Server Members Intent** (required for permission checking)
   - ☑️ **Message Content Intent** (required for channel operations)
3. **Click "Save Changes"**

> **Why needed:** The bot checks user permissions (admin, voting role) and manages channel messages for election announcements.

### Generate Bot Invite URL
1. Go to "OAuth2" → "URL Generator"
2. **Select Scopes:** 
   - ☑️ `bot`
   - ☑️ `applications.commands` 
3. **Select Bot Permissions:**
   - ☑️ Send Messages
   - ☑️ Read Message History  
   - ☑️ Manage Messages
4. **Use the invite URL:** Open the generated URL in your browser, select your server, click "Authorize"

### Discord Server Requirements
- **Create channel:** `#announcements` (bot posts election updates here)
- **Create role:** `voting` (no special permissions needed - just assign to trusted admins)
- **Get Server ID:** 
  1. Enable Developer Mode: Discord Settings → Advanced → Developer Mode ☑️
  2. Right-click your **server name** (in left sidebar) → "Copy Server ID"

## ⚙️ Bot Configuration

### Environment Setup
```bash
cp .env.example .env
```

Edit `.env` with:
- Discord bot token 
- Your server ID (the number you copied above)
- Optional: customize role names, timing, etc.

## 🚀 Deploy

```bash
docker compose up -d
```

Check logs: `docker compose logs -f`

## ✅ Testing

1. **Start election:** `/start prompt:Test election` (requires `voting` role or admin)
2. **Add proposals:** `/prop idea:pizza party`
3. **Check suggestions:** Bot should update announcements channel
4. **Stop to begin voting:** `/stop`
5. **Cast votes:** `/vote n:3 id:0` (3 votes for option #0, costs 9 credits)
6. **Check points:** `/points` (shows remaining voice credits)

## 🚨 Common Issues

- **Bot crashes with "DisallowedGatewayIntents":** Enable privileged intents in Discord Developer Portal (see step above)
- **Bot won't start:** Check `DISCORD_SECRET` in `.env`
- **No responses:** Verify server ID in `APPROVED_SERVERS` and rebuild
- **Permission errors:** Ensure bot has required channel permissions
- **Slash commands not appearing:** Wait a few minutes for Discord to register them, or re-invite the bot
- **Commands ignored:** Make sure `#announcements` channel exists

---

*Ready to vote! (◕‿◕✿)*
