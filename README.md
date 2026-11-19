# activator-nexus

Source preparation for Proposal 2: a session-owned, target-local activator. No host-specific or user-specific wire type. The profile itself supplies Live; no activation history is held here.

The initial body validates the generation, executes activation in its inherited service environment, publishes the configured profile only after success, and checks the profile before answering Activated. Command executables, arguments, store and profile paths are supplied as configuration. It starts neither a user session nor an SSH process.

This source is not a released service. Serving sockets, configuration lifecycle, the two clients, systemd packaging, Lojix integration and reconciliation, bounded gates and independent audit remain to be implemented. The remote Lojix deployment route is preserved until a replacement is delivered. No build has been run under this branch.
