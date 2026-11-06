# krebs-matchrun

A mod for Krebs, which is on [GitHub](https://github.com/DefaultO/krebs) and [Gitea](https://gitea.defaul.to/DefaultO/krebs).
Either one is enough. It lives at `mods/matchrun` in the core, as a git submodule, and it builds only inside the core. To get it:

```
git clone --recursive https://github.com/DefaultO/krebs.git
```

It runs only inside a match's own server, which `mods/bare` starts with `--mods devtools,matchrun`. Never turn it on for the
main server. It sets a bot's CPU limit (20, plus 10 per GCL level, up to 300, on a full bucket), lifts the 5 MB limit on a
bot's code, and adds `/api/matchrun/view` and `/api/matchrun/rooms`, which the landing page draws while a match plays.

The code is under the MIT licence, see `LICENSE`.
