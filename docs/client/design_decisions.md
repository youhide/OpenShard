# The decisions this client is built on

Seven standing decisions, each of which is cheap to honour from the first
triangle and an audit to retrofit. They are here rather than in the domain
README because they constrain every document beside them: what we draw with, what
the browser costs us now, that colour is never converted, and that the claimed
client version belongs to a shard rather than to the process.

## Decisions to take before they are taken by accident

- **The client is multi-shard and multi-session**, `[shard → {characters}]` —
  see M3b. It is a decision and not a feature, because it says three things that
  are cheap now and an audit later: everything downstream of a socket is per
  session, the data files are loaded once *per install* rather than once per
  process, and the claimed version belongs to a shard rather than to the client.
  The same argument as multi-era on the server, and for the same reason.
- **Crates.** `crates/client/net`, `crates/client/render`, `crates/client/app`,
  plus `crates/common/uofiles`. The direction rule stands: a client crate
  depends on `common`, never on `server`.
- **What we draw with.** `wgpu`, directly — no engine. Bevy and its neighbours
  would supply a window, input and sprite batching, and none of what is actually
  hard here: UO's draw order, the hue table, and streaming map blocks out of a
  155MB container. What they *would* impose is a second ECS beside `WorldView`,
  ownership of the main loop, and a frame that cannot be rendered inside
  `cargo test`. ClassicUO does not use an engine either, and not out of poverty.
- **The browser is a target, so it constrains the design now.** WebGL2's ceiling
  rather than native Vulkan: no compute, no storage buffers, instancing through
  vertex buffers, a 2048 atlas, and `async` device requests because a browser
  cannot be blocked on. Cheap to honour from the first triangle and painful to
  retrofit. What is *not* done: `uofiles` still opens paths, and a browser has
  no filesystem — the parsing is already separate from the reading, so the fix
  is byte-taking constructors and `std::fs` behind `cfg`, not a rewrite.
- **Colour is never converted.** Textures and targets are `Rgba8Unorm`, never
  `…Srgb`. The files hold five bits a channel with no colour space attached, and
  a gamma conversion anywhere means the pixel that went into the atlas is not
  the pixel in the frame — which turns every exact test assertion into a
  tolerance nobody can justify.
- **Which version we claim to be.** The client announces one in its seed, and
  every `Feature` gate on the server follows from it. 7.0.45.65 — what
  ClassicUO opens with — keeps us on the modern packet set instead of the legacy
  branches of every encoder. **Per shard, not per client** (M3b): the whole point
  of `Feature::since` is that a connection asks its own version, and a client
  facing two shards at two versions is exactly the case an era check gets wrong
  silently.
- **A client that only speaks what our server happens to send is a mirror, not
  a UO client.** That is the right scope for M1–M3, and it is also how both
  ends quietly agree on the same mistake. Every packet this client learns should
  be checked against a real client's behaviour, not only against our own server.

