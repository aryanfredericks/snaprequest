import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";
import { theme } from "./lib/state/theme.svelte";

// Before the first render, so the page does not flash in the other theme.
theme.apply();

const app = mount(App, { target: document.getElementById("app")! });

export default app;
