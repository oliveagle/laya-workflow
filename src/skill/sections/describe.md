# describe — print the graph of a built-in app

`laya-workflow describe <app>` prints the workflow graph as JSON:
`start`, `max_iterations`, and each node's `questions`, `primary_q`,
`edge_condition`, `edge_default`, `min_confidence`, `has_action`, `max_retries`.

Use it to understand an app's topology before running it, or as a reference when
you `export` it to a generic spec and edit it.

Next: `skill --section apps`, `skill --section export`.
