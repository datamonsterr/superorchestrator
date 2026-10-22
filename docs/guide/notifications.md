# Notifications

The supervisor can tell you when something needs you. It sends a Discord message, a desktop notification, or both.

## Events

Set the events in `notifications.on`. The default is all four:

| Event | When |
|---|---|
| `escalated` | A run stayed stalled after three nudges |
| `question_waiting_10m` | A subagent's `ask_parent` question has had no answer for 10 minutes |
| `session_finished` | A session ended |
| `account_exhausted` | An account or pool hit a quota or rate limit |

## Discord

Create a webhook in your Discord server. Put its URL in an environment variable of the supervisor. The default name is `SO_DISCORD_WEBHOOK`:

```sh
export SO_DISCORD_WEBHOOK='https://discord.com/api/webhooks/...'
```

Set `notifications.discord_webhook_env` to use another name. The URL is a secret. It is read from the environment, never from `config.toml`, and never logged. Start the supervisor after you set the variable.

## Desktop

`notifications.desktop = true` (the default) uses `notify-send` on Linux and `osascript` on macOS. Install `libnotify` if `notify-send` is missing. Set it to `false` to turn desktop notifications off.

## What a notification says

A notification names the event, the run, and a short reason. It never carries task text, results or secrets.

## Rate limit

There is at most one notification per run per event type every 10 minutes.

## Example

```sh
superorchestrator config set notifications.on '["escalated", "account_exhausted"]'
superorchestrator config set notifications.desktop false
```
