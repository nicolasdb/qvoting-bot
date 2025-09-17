# 🧠 Voting Logic & Governance

This document explains the democratic governance model implemented by the quadratic voting bot.

## 🏛️ Governance Roles

### 👑 Election Administrators (`voting` role)
**Powers:**
- Start new election cycles with `!start "topic"`
- Force-stop any phase early with `!stop` 
- Control election timing and flow

**Cannot:**
- Vote with extra power (same 100 credits* as everyone)
- Propose ideas during voting phase
- See who voted for what (votes are aggregated)

> _*credits: the max amount is set in .env during deployment_

**Design Intent:** Trusted facilitators who manage process, not outcomes.

### 👥 Community Members (everyone else)
**Powers:**
- Propose ideas during suggestion phase: `!prop "my idea"`
- Vote during voting phase: `!vote 3, 0` (3 votes for option 0)
- Check their remaining credits: `!points`

**Cannot:**
- Start/stop elections
- Propose during voting phase
- Vote during suggestion phase

---

## 🔄 Election Lifecycle

### Phase 1: Suggestion Period
```
State: Open for proposals
Who can act: Everyone (except admins managing flow)
Available commands:
  !prop "idea"     ← Add new proposal
  !stop           ← Admin only: end phase early
```

**Logic:**
- Anyone can propose unlimited ideas
- Duplicates are rejected automatically
- Admin can end phase early if needed
- Bot updates announcement with live proposal list

### Phase 2: Voting Period
```  
State: Proposals locked, voting open
Who can act: Everyone (except admins managing flow)
Available commands:
  !vote 3, 1      ← Cast 3 votes for option #1
  !points         ← Check remaining credits
  !stop           ← Admin only: end election early
```

**Logic:**
- Each person gets exactly 100 voice credits
- Quadratic cost: n votes = n² credits
- Can change votes on same option (refunds previous cost)
- Bot updates announcement with live results

### Phase 3: Results & Reset
```
State: Election complete, preparing for next cycle
Who can act: Admin (to start next election)
Available commands:
  !start "topic"  ← Begin new election cycle
```

**Logic:**
- Winners announced in order of vote totals
- All credits reset to 100 for everyone
- Vote history cleared
- System ready for next election

---

## 🎮 Quadratic Voting Mechanics

### Credit Economics
```
Everyone starts with: 100 credits
Vote cost formula: votes² = credit cost

Examples:
1 vote  = 1 credit   (efficient for mild preferences)
2 votes = 4 credits  
3 votes = 9 credits
5 votes = 25 credits (expensive for strong preferences)
10 votes = 100 credits (all-in commitment)
```

### Strategic Implications
- **Spread strategy:** Many small votes across options
- **Focus strategy:** Fewer large votes on priorities  
- **Impossible to dominate:** Even with 100 credits, max 10 votes per option
- **Intensity matters:** Strong preferences cost exponentially more

### Vote Changes & Refunds
```
Scenario: User already voted 2 votes (4 credits) for Option 1
Action: !vote 5, 1 (wants to change to 5 votes)
Process:
  1. Refund previous: +4 credits (back to 100)
  2. Charge new cost: -25 credits (5² = 25)
  3. Net result: 75 credits remaining
```

---

## 🔮 Potential Enhancements

### Governance Modifications
- **Proposal Limits:** Max proposals per person per election
- **Voter Eligibility:** Role-based voting restrictions
- **Weighted Credits:** Different starting credits by role/tenure
- **Anonymous Proposals:** Hide proposal authors during suggestion phase

### Economic Tweaks  
- **Credit Carryover:** Unused credits partially saved for next election
- **Dynamic Pricing:** Vote cost changes based on total participation
- **Bonus Systems:** Extra credits for consistent participation
- **Decay Model:** Credits expire if unused for multiple cycles

### Process Improvements
- **Ranked Phases:** Multiple voting rounds with elimination
- **Hybrid Timing:** Flexible phase durations based on activity
- **Approval Voting:** Yes/no on multiple options instead of allocation
- **Delegation:** Allow credit transfer between trusted users

---

*Democracy is a technology - and this is one implementation of it!* (◕‿◕✿)
