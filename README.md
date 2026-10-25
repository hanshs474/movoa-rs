# movoa

Unlimited AI video generator with sound: on paid plans the tenth take of a shot costs what the first did, and your first clip is free to make right now. This is the official Rust SDK for [Movoa](https://movoa.ai/?utm_source=crates.io&utm_medium=package): one API key turns a prompt into [text to video](https://movoa.ai/video/text-to-video), or a still into [photo to video](https://movoa.ai/video/photo-to-video), and every clip comes back as [AI video with audio](https://movoa.ai/video/ai-video-with-audio).

```rust
let clip = movoa::generate_video("a paper lantern drifting over a night river, slow dolly", &movoa::Options::new())?;
println!("{clip}"); // URL of the finished MP4, sound included
```

`generate_video` submits the run, waits for it and returns the URL of the finished file.

## Install

```bash
cargo add movoa
```

Blocking client on `ureq` and `serde_json`; no async runtime needed.

## API key

Sign in on [movoa.ai](https://movoa.ai/?utm_source=crates.io&utm_medium=package), create a key at [movoa.ai/settings/apikeys](https://movoa.ai/settings/apikeys), then export it:

```bash
export MOVOA_API_KEY=sk-...
```

Runs cost credits on your Movoa account, at the prices on [movoa.ai/pricing](https://movoa.ai/pricing).

```rust
let left = movoa::credits()?; // remaining credits on the account
```

## Text to video and photo to video

```rust
use movoa::Options;

// text to video, vertical, 10 seconds
movoa::generate_video("a fox trotting through fresh snow at dawn", &Options::new().set("duration", 10).set("aspect_ratio", "9:16"))?;

// photo to video: the photo becomes the first frame
movoa::generate_video("the camera slowly orbits the statue", &Options::new().image_url("https://example.com/statue.jpg"))?;

// another model
movoa::generate_video("a street drummer in the rain", &Options::new().model("seedance-2-5").set("resolution", "720p"))?;
```

| model | options |
|---|---|
| `minimax-h3` (default) | `duration` 4 / 5 / 6 / 8 / 10 / 15 · `resolution` 480p / 768p · `aspect_ratio` 16:9 / 9:16 / 1:1 |
| `veo-3-1-fast` | `duration` 8 · `resolution` 720p · `aspect_ratio` 16:9 / 9:16 (photo to video: 16:9) |
| `seedance-2-5` | `duration` 5 / 10 / 15 / 20 / 25 / 30 · `resolution` 480p / 720p · `aspect_ratio` 16:9 / 9:16 / 1:1 |
| `seedance-2`, `seedance-2-fast`, `seedance-2-mini` | `duration` 4 / 6 / 8 / 10 · `resolution` 480p / 720p · `aspect_ratio` |

Every run asks for sound. On the site: [Seedance 2.5](https://movoa.ai/video/seedance-2-5) and [Veo 3](https://movoa.ai/video/google-veo-3).

## Submit now, collect later

```rust
let c = movoa::Client::from_env()?;
let id = c.submit("a lighthouse in a storm", &movoa::Options::new())?;
let task = c.status(&id)?;   // task.status: pending | processing | success | failed | canceled
let url = c.wait(&id)?;      // polls every 10 seconds, 30 minutes by default (c.poll_every, c.timeout)
```

While a run waits in line, `task.queued` is true and `task.queue_eta` holds the seconds.

## Errors worth matching

- `Error::Auth`: no key, or the key is wrong or deleted; create one at [movoa.ai/settings/apikeys](https://movoa.ai/settings/apikeys)
- `Error::Quota`: the account does not have enough credits for this run
- `Error::Plan`: that model or quality needs a plan on the account
- `Error::Busy`: too many runs on this account are generating at once
- `Error::Rejected`: the content filter refused the prompt, or a cooldown is active; reword it
- `Error::Generation { error_message, .. }`: the run was accepted but failed or was canceled
- `Error::Timeout { task_id }`: the wait ran out; check the run later with `status`
- `Error::Other`: any other refusal, with the server's message

MIT licensed. Made by [Movoa](https://movoa.ai/?utm_source=crates.io&utm_medium=package).
