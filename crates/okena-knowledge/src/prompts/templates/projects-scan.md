---
name: projects-scan
description: Brief an agent to find the links between several repositories and record each in both of their project maps
for: projects-scan
---
Find how these repositories connect, and record every link in both of their project maps.{projects}

Follow the project-map skill in `{skill}`, and read its section on links before you start. For each repository, read its `project-map.yaml`, then the code where it calls, publishes to, reads from or is called by another repository on the list. When a link is real, write it under `links` in both maps: `direction: uses` in the map of the repository that uses the other, `direction: used_by` in the other's, with the same `type` and `name`. Add the matching `exposes` and `consumes` entries where they are missing. A repository without a valid map needs one first: map it as the skill describes.

Leave what you write in each checkout for me to review. Do not commit or push.

{>reporting}
