# Showcase

`kurogane showcase` runs Kurogane's showcase application. It fetches [kurogane-showcase](https://github.com/kurogane-rs/kurogane-showcase) into Kurogane's cache folder and starts it with `kurogane dev`. The build directory stays between runs. A run that cannot fetch the showcase uses the last copy.

The showcase is a Rust application that owns its window and its event loop. Chromium is one part of the frame:

* **Galaxy:** A wgpu compute simulation of millions of stars drawn into the application's own window
* **Panes:** Three live Chromium pages inside the same window. One steers the galaxy, one plots it and one shows what the loop reports.

Everything runs on one thread in one winit event loop. Chromium is pumped only when it asks to be.

## Controls

* The sliders in the controls pane set the star count, gravity, swirl, turbulence, color, camera spin and star size
* **Burst** or `Space` pushes the stars outward for a moment
* **Pull apart** or `E` separates the window into its layers

## Notes

The showcase demonstrates:

* Chromium pages as child browsers of a native window (`AppInstance::create_child_browser`)
* Pumping Chromium from the application's own loop (`App::scheduler`)
* Commands, streams and events between the pages and Rust
