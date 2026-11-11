# world: an open world to walk through

An exploration game with no edge: walk, run and look around a landscape of lakes, shores, hills and snow-capped peaks under a sun that crosses the sky
(a day takes eight minutes), all of it painted in broad brush strokes. The whole picture is made by one fragment shader on the GPU; the program keeps only where
you stand and the time of day.

```
mito build examples/world/world.mito -o world            # SDL3.dll (3.2 or newer) must be beside it or on the path
world                                                    # W A S D walk, Shift runs, the mouse looks, Escape leaves
world --frames 1 --screenshot frame.ppm --size 1280x720  # no window: render one frame to an image
```

| File | |
| --- | --- |
| `world.mito` | the state (the player, the seed, the time of day), `@start`, the two `@shader`s, and `@frame`: looking, walking, the clock, and the frame's two passes |
| `scene.frag` | the world as a camera sees it, drawn small: a ray marched over a height field of fractal noise until it meets the ground; lakes, sand, grass, rock and snow by height and slope; sun, shadow, fog and sky |
| `paint.frag` | the painting: reads the small picture and lays it down in brush strokes over the window |

## What it shows

- **A game as an ordinary program.** `platform game;`, an `@frame` function whose inputs are what the window saw (`dt: f64 from time`, `forward: bool from key("w")`, `look_x: f64 from mouse("dx")`),
  and `@shader` functions that give the shaders' sources. The frame's answer is a line for each pass: `scene@0.25 px=330 pz=60 yaw=0.35 pitch=-0.03 day=0.10 seed=111` draws the world into an image a quarter of the window's size, and `paint` draws the window from it.
- **Nothing stored.** The ground exists everywhere because the shader computes it from noise and the seed: there is no map, no chunks and no edge, and the same place always looks the same.
  Change `world.seed` and the world is another one. The state is six numbers.
- **Painted, not photographed.** The picture is three layers of strokes, big under small, each a long narrow cell that follows a slowly turning flow field and bends like a wrist. A stroke is one flat
  colour, the colour of the world at the stroke's middle (so a stroke that crosses an edge takes one side), streaked along its length by bristles, a little darker where the paint thins at its edges, with warm paper
  showing through at the margins. Small distant detail dissolves into strokes: that is what broad brushwork does. The knobs are the stroke sizes, the flow's amount and the three `strokes(...)` calls in `main`.
- **Rules in the language, picture on the GPU.** Walking along the direction you face is `sin` and `cos` of the yaw; looking up and down is clamped with `max(-1.4, min(1.4, ...))` on floats.
- **Seeing it without a window.** `--frames N --screenshot f.ppm` renders in a window that is not shown and saves the last frame. `cargo test game_renders_the_world -- --ignored` (with `MITO_SDL3_DIR` set) builds this
  example, renders it and checks the picture.

Speed: the world is drawn once into an image a quarter of the window's size and the strokes read their colours from it, so the expensive part (a ray march for every pixel) runs on a sixteenth of the pixels. On an
integrated Intel Arc GPU a frame takes about 1.2 ms at 1280x720, 3.2 ms at 1920x1080 and 7.6 ms at 2560x1440 (measured as wall time over 120 frames with vsync off). Drawing the world at full size for every stroke layer
that covers a pixel took 28 ms and 50 ms at the first two sizes.

Not here: collision (the camera follows the height of the ground, or of the water, so you walk across lakes), objects, sound, saving the player.
