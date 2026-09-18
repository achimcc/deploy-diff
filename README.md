# deploy-diff

Before a NixOS deploy: names what the new generation takes away from the one
that is running — and stops the deploy when the loss comes from a stale tree.

A deploy from a stale tree does not fail. It activates, exits 0, and the host
runs a generation without the guest, the unit or the secret another session
rolled out an hour earlier. A removed service is not a failed one:
`systemctl --failed` stays empty, the deploy is green, and nobody finds out
until someone clicks on something. In the homelab this tool comes from, that
happened to the same container four times in one morning, and two deploys
from an outdated `flake.lock` quietly put rotated secrets back.

```console
$ deploy-diff compare /run/current-system /nix/store/…-nixos-system-server --live --stale
deploy-diff: 34 system(s) running, 33 after — 1 guest(s), 3 unit(s), 0 input(s) lost
  LOST  guest req-01 would be removed
  LOST  host  br-req-netdev.service would stop
  LOST  input homeserver-secrets goes back: 3f1c02aa (2026-09-12) -> 91d0e7b4 (2026-09-03)
STOP: the tree does not contain the running commit — this deploy takes away what another one rolled out.
Merge the running commit, or let it through on purpose (--accept).
```

## What counts as a loss

Compared are two toplevels: the running one and the one about to be
activated, together with their declarative containers
(`etc/nixos-containers/*.conf`).

- **A guest** the new generation no longer declares. Its units are not listed
  one by one.
- **A unit** of the host or of a remaining guest — services, sockets, timers,
  paths, mounts, automounts, swaps. Targets, slices and scopes only group
  other units. An instance counts through its template
  (`mail@x.service` survives while `mail@.service` exists); a unit the new
  generation masks is lost.
- **A flake input that goes back in time**, by `lastModified`. This needs the
  generation to record its inputs in `etc/flake-inputs.json`:

  ```nix
  environment.etc."flake-inputs.json".text = builtins.toJSON (
    builtins.mapAttrs (_: i: { rev = i.rev or null; lastModified = i.lastModified or 0; })
      (builtins.removeAttrs inputs [ "self" ])
  );
  ```

  A generation without the file is reported as not comparable, not as a loss.

With `--live` (as root, on the target) only what runs counts: active units of
the host and of each running container (`systemctl -M`), and guests that run.
A running guest whose systemd does not answer (booting, shutting down) falls
back to its declared units and is named in a note — it does not fail the
comparison. Without `--live`, everything the old generation declares counts — that is how the
history below was measured. `--live` also reports when `/run/current-system`
is not the system profile: the residue of a `switch-to-configuration test`.

## Warn or stop

A loss from a tree that **contains the running commit** was decided in that
tree — a guest abolished, a unit renamed. It is a warning, exit 0.

A loss from a tree that **does not** is what another deploy rolled out, taken
away again. It stops the deploy, exit 1 — unless `--accept`: a rollback is
exactly this, on purpose.

deploy-diff does not run git; the caller decides and passes `--stale` when the
running commit (`nixos-version --configuration-revision`) is not an ancestor of
the tree's `HEAD`, is unknown locally, or was built from a dirty tree — its
content is then in no commit at all.

## Measured before use

Every rule was run over the full history of the host it was written for:
859 generations, 858 transitions, without `--live`.

| | transitions |
|---|---|
| would have stopped | 48 |
| … whose losses came back in a later generation (a real revert) | 41 |
| … whose losses were deliberate, from a stale tree (a merge resolves it) | 7 |
| warned, with losses | 15 |
| … whose losses came back later (a miss) | **0** |

All seven documented incidents — a guest removed by a stale deploy — stop; the
one deliberate removal of a guest warns.

## Build and use

```console
$ nix build github:achimcc/deploy-diff
$ deploy-diff compare OLD NEW [--live] [--stale] [--accept]
```

Exit 0: nothing lost, a warning, or accepted. Exit 1: losses from a stale
tree. Exit 2: an error — a toplevel that cannot be read, systemd that does not
answer.

The new generation is on the build machine before activation, the running one
only on the target. The intended flow copies the new closure to the target
first (`colmena apply push`, or `nix copy`) and runs the deploy-diff *of the
new generation* there — so the tool never judges with an outdated version of
itself.

The unit loader is shared with [unit-lint](https://github.com/achimcc/unit-lint).

## License

AGPL-3.0-only.
