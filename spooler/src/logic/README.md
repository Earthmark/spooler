# Logic

The logic module allows items to contain javascript and wasm scripts.

## Architecture

The logic service runs in a frontend `Logic` component, and a backend stack that communicate via message passing.

* Manager - Manages workers.
* Worker - A thread that may contain multiple contexts, this schedules runtimes to execute.
* Runtime - A single instance of logic that does something.
* Feature - A set of APIs and systems a script can interact with.
* FeatureInstance - A live set of APIs that deal with updating the API data in the runtime.

A `RuntimeConnection` component is used to send and recieve events between a `LogicRuntime` and `Logic` component.
