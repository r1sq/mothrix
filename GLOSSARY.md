# mothrix

A Matrix client with two front ends, the terminal and the iPhone app,
over one core. These are the words its code and docs use.

## Accounts and rooms

**Account**:
One signed-in Matrix user, kept under a profile. mothrix shows several at once.
_Avoid_: user (for the signed-in one), session

**Profile**:
The name an account's files are kept under; `default` for the first one.

**Room key**:
How the front ends know a room: its room id, with `profile/` in front for every profile but `default`. Never the same thing as a room id.
_Avoid_: room id (for the key)

**Open room**:
The one room whose conversation is shown; at most one over all accounts.

**Forwarding**:
Sending a copy of a message into another room, of the same account or another one.

## The room list

**Own section**:
A group of rooms the user named and filled. It travels with the accounts, so every device lists rooms the same way.
_Avoid_: folder, category, tag

**Subsection**:
An own section inside another one (`Parent/Name`); one level deep at most.

**Share**:
The part of the own sections an account stores: every section that holds its rooms or none at all, with only its rooms.

**Fold state**:
Which sections and accounts of the room list are folded. Kept per device, not with the account.

## Front ends

**Front end**:
What the user sees and touches: the terminal client or the iPhone app.
_Avoid_: UI (for one of them), frontend

**Client state**:
What both front ends know about the accounts and rooms (the open room, own sections, fold state, unread counts, when to notify), apart from how they show it.
