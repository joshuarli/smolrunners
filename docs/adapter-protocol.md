# Adapter protocol

The coordinator runs the Go adapter as a child process. The two exchange length-prefixed JSON
frames: a 4-byte big-endian length, then a UTF-8 JSON body of at most 1 MiB with
`schema_version: 1` and a `kind` tag. The adapter's stdout carries adapter→coordinator frames.
Its stdin carries coordinator→adapter frames. Stderr is a log.

## Start

The coordinator writes one `run` frame first:

```json
{"kind":"run","schema_version":1,"id":"run-1","scenario":"connected-a"}
```

`scenario` is used only in fake mode and is empty in GitHub mode. The adapter's mode comes from its
command line, not from this frame:

| Invocation | Mode |
| --- | --- |
| `smolrunners-adapter --fake` | Fake broker. Scenarios load from `SMOLRUNNERS_SCENARIOS_DIR`. |
| `smolrunners-adapter --github-config FILE` | Real `actions/scaleset` client. |

## Events (adapter → coordinator, acked)

The adapter submits each event and blocks until the matching `ack`
(`event_id` equals the event's `id`, `recorded: true`). The kinds are `capacity`,
`job_available`, `acquire_outcome`, `job_assigned`, `job_started`, `job_completed`, `vm_delete`
and `session`. `job_completed` carries `runner_name` when GitHub supplies it. A final `session`
frame with `session_event: "adapter_stopped"` ends the stream.

Event IDs are derived from the GitHub message ID, so a redelivered message produces the same
IDs, and the coordinator answers a replay with its recorded decision.

## Commands (coordinator → adapter, answered)

The coordinator may send a command frame at any time. The adapter answers each command with one
`command_result` whose `command_id` equals the command's `id`. Results need no ack. The adapter
must run commands concurrently with its listener loop, and a reader goroutine routes `ack`
frames to the waiting submitter.

```json
{"kind":"jit_request","schema_version":1,"id":"cmd-7","runner_name":"smol-sr-host-a-1732"}
{"kind":"remove_runner","schema_version":1,"id":"cmd-8","runner_id":42}
{"kind":"command_result","schema_version":1,"id":"res-cmd-7","command_id":"cmd-7","ok":true,
 "runner_id":42,"encoded_jit_config":"<base64>"}
{"kind":"command_result","schema_version":1,"id":"res-cmd-8","command_id":"cmd-8","ok":false,
 "error":"remove runner 42: 404"}
```

`jit_request` calls `GenerateJitRunnerConfig` for the scale set with that runner name.
`remove_runner` calls `RemoveRunner`. A runner that GitHub has already removed (404) answers
`ok: true`. `encoded_jit_config` is a credential: neither side logs it or writes it to durable
state.

In fake mode, `jit_request` returns a syntactically valid fake JIT config: base64 of a JSON object
whose `.runner`, `.credentials` and `.credentials_rsaparams` members are base64 JSON. The
`.runner` `serverUrl` points at `http://127.0.0.1:9`. Each fake runner gets a sequential
`runner_id`. A scenario step may name `"runner_name": "$jit"`, which the fake broker replaces with
the name of the most recent `jit_request`. A step may also set `after_ms` to delay its delivery.

## Scaling

The coordinator, not the adapter, decides how many runners exist. On each `capacity` event it
starts runners until the live count reaches
`min(max_concurrency, assigned_jobs)`. Each runner gets a fresh JIT config through
`jit_request`. Jobs join their execution by `runner_name` on `job_started` and `job_completed`.
