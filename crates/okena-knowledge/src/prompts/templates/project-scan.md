---
name: project-scan
description: Brief an agent to write or update a repository's project map
for: project-scan
---
Map the repository {project} at `{path}`: the areas its code is divided into, the concepts they implement, what it exposes to and consumes from other projects, and how it is built and run.

Follow the project-map skill in `{skill}`. Read all of it before you start: it says what to extract, which docs to write, and the exact shape of `project-map.yaml`. Write the map into `{map_root}`.{start}

Leave what you write in the checkout for me to review. Do not commit or push.

{>reporting}
