roughly highest to lowest priority, though order may change at any time

to v0.1:
- support `modifyOtherKeys`; kitty moves up to rank 2 and `modifyOtherKeys` becomes rank 1. raised from v0.2 because most of the combinations that currently error with "needs kitty" (ctrl+h, ctrl+shift+a, ...) really only need `modifyOtherKeys`, and because the modifier pattern shorthands below wait on it
- rework the modifier pattern shorthands alongside `modifyOtherKeys`: rename `any` to `catchall`, add `most` (shift/alt/ctrl/meta) and `every` (all known modifiers; named so it doesn't read as a synonym of `catchall`). they have to be combination sets rather than alternation lists, since `none || shift || ctrl` doesn't catch shift+ctrl - so either special-case them the way `any` is, or invent a general "any combination of these modifiers" operator. `most` can't be satisfied by legacy for text sources at all, which is why it's coupled to `modifyOtherKeys` rather than standing alone
- write a new readme from scratch; should probably not be too reference-y, document the basics and the common "remap some keys" usecase but leave the details to the manpage
- bring some more consistency to the timeouts/etc scattered across various configurable options and constants and such, and add a `--high-latency` preset; the two realistic modern usecases are a terminal emulator, which has a tiny near negligible latency, and a ssh/network connection which can have a very high latency if ping is high
- print include chain in diagnostics for non-root files
- group cycle / include cycle formatting
- consider making a breaking change and make mapping target keys that cannot be encoded for the child's downstream protocol (not the config's selected protocol) disallowed, as the alternative of having it silently do nothing because the child doesn't understand it is worse. the thing, though, is that the child can change its wanted protocol at any time, so it can't just be an upfront config scan; maybe don't do a breaking-change rejection instead and just log a warning? or maybe only allow non-legacy mapping targets with `maybe_lossy!`?
- define and document a deterministic ordering between multiple mappings with the same source
- manpage; write in markdown and have the md file in the repo, convert to a manpage with something like pandoc
- `unique_src!` to catch "why does this produce multiple keys"; maybe a boolean option `unique_sources`, and if it's on then you can make a source non-unique with `non_unique_src!`
- implement `@protocol require`

to v0.2:
- consider reserving `unsafe!` for things like `bytes(...)` / `raw_passthrough!` / etc.; `unsafe!` would presumably be for anything that goes outside of the "child and config are blind to the actual negotiated terminal mode / the actual bytes being exchanged" model
- text-input mapping target; coupled with deterministic ordering between same-source mappings, this could allow for things like `key('\'~) => send_key(esc)`, `key('\'~) => send_text(":cd ")`. technically already doable character-by-character but tedious
- maybe some kind of facility for keyboard layout agnostic input with the kitty protocol
- byte literals, byte arrays, and `sockdata_bytes(...)` / `unsafe! bytes(...)`
- consider a general "set variable/flag" + "conditional mapping" system; it would supersede `toggle_mappings(...)` and `always!`, which can then just be removed (bumping the format to `@version 2` if that lands after prerelease)
- consider doing a dfs/kahn to find group cycles at config load rather than at runtime
- think of more things to add here
