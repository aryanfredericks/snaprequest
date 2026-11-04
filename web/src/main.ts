import { mount } from "svelte";
import Root from "./Root.svelte";
import "./app.css";
import { theme } from "./lib/state/theme.svelte";

// Before the first render, so the page does not flash in the other theme.
theme.apply();

const app = mount(Root, { target: document.getElementById("app")! });

export default app;
