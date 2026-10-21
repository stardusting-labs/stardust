# Contributing

Stardust is licensed under Apache-2.0. By contributing, you agree that your contributions are under that license.

Run the same checks as CI from a clean checkout:

```sh
./scripts/ci.sh
```

The script formats, lints, tests, checks licenses, and scans for secrets. Install the secret scan as a hook with:

```sh
ln -sfn ../../scripts/pre-commit .git/hooks/pre-commit
```

User-facing wording is checked by the copy test in `crates/stardust-core`. Keep the README, this file, and `SECURITY.md` consistent with that test.

Dependencies must use a license listed in `deny.toml`. The allow list rejects unlicensed crates and AGPL licenses. Do not add a crate outside that list without updating the file and the reason.

The only host that belongs in product text and examples is `https://helloorbit.app`. Do not commit secrets, tokens, or private host names.
