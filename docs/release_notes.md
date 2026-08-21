# Release notes

## Unreleased

### Building privacy

The **Rooms and floors** view now treats the building the player occupies as a
single private context.

- Move between the building's real structural floors and inspect the selected
  storey without switching to a global height slice.
- The current building remains a whole object: its rooms, open-door reachability,
  short stairs, and stage platforms resolve together.
- Other buildings in view are withheld, including their interior space and
  nearby roof/wall shell art, so neighbouring houses cannot leak into the
  current building view.
- A low wall that directly supports a stage platform is treated as the
  platform's riser rather than a second room or a separate building.
