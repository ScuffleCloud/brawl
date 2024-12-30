# Brawl Server Configuration

This file refers to the `config.toml` used by the server. If you are looking for a repository configuration reference, please refer to the [**`repository configuration reference`**](./repository-configuration.md).

## Level

The `level` option is used to set the logging level. This can be one of the following values:

- `trace`
- `debug`
- `info`
- `warn`
- `error`

It can also be a compound value using the tracing [**`env-filter syntax`**](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives).

## Telemetry Bind

The `telemetry_bind` option is used to set the bind address for the telemetry server. This is useful if you want to run the server in a container and want to expose the telemetry endpoint.

By default this is not set, so the telemetry server is not started.

However if the telemetry address is provided a server will listen on the address and expose a `/metrics` endpoint as well as a `/health` endpoint.

The health endpoint runs a check to see if the database is healthy and can be reached.

The metrics endpoint exposes prometheus metrics used for monitoring the server.

## Database URL

The `db_url` option is used to set the database URL. This is the URL of the database that brawl will use to store its data.

This defaults to the `DATABASE_URL` environment variable, if it is not set brawl will not be able to run.

This should be set to a valid postgres database url; for example:

```
DATABASE_URL=postgres://brawl:brawl@localhost:5432/brawl
```

## Interval Seconds

The `interval_seconds` option is a refresh rate used by brawl to refresh PRs and check for timeouts.

This defaults to 30 seconds.

## GitHub

The `github` section is used to configure the GitHub app.

This section is required.

### Webhook Bind

The `webhook_bind` is a bind address to run the webhook server on.

This defaults to `0.0.0.0:3000`.

### App ID

The `app_id` is the ID of the GitHub app.


This is required.

### Private Key PEM

The `private_key_pem` is the private key of the GitHub app. This should not be encoded in base64, it should be the raw private key.

You can generate a private key by going to the GitHub App settings page under the `General` tab and scrolling down to the `Private key` section. For more information look at the [**`Setup Guide`**](./setup.md#5-generate-a-private-key).

This is required.

### Webhook Secret

The `webhook_secret` is the secret used to verify the webhook.

This can be configured on the GitHub App settings page under the `Webhook` section. For more information look at the [**`Setup Guide`**](./setup.md#2-provide-a-webhook-url).

This is required.
