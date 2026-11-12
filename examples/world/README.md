# world: an open world to walk through

An exploration game with no edge: walk, run and look around, following a small figure through a landscape of lakes, shores, hills and snow-capped peaks under a sun that crosses the sky
(a day takes eight minutes), all of it painted in broad brush strokes. The whole picture is made by one fragment shader on the GPU; the program keeps only where
you stand and the time of day.

```
mito build examples/world/world.mito -o world            # SDL3.dll (3.2 or newer) must be beside it or on the path
world                                                    # W A S D walk, Shift runs, the mouse looks, Escape leaves
world --frames 1 --screenshot frame.ppm --size 1280x720  # no window: render one frame to an image
```

| File | |
| --- | --- |
| `world.mito` | the state (the player and its figure's heading and stride, the seven orbs and which are collected, the seed, the time of day), `const Tune` with the tuning numbers, `@start` (which places the orbs from the seed with `Rng`), the two `@shader`s, `@frame`: looking, walking, collecting, the clock, and the frame's two passes, and `test` blocks for the rules |
| `scene.frag` | the world as a camera sees it, drawn small: a ray marched over a height field of fractal noise until it meets the ground; lakes, sand, grass, rock and snow by height and slope; sun, shadow, fog and sky; and the walking figure (signed-distance capsules and spheres, marched only inside a sphere around it) with a blob shadow; it writes two pictures, the light and a second one of depth, normal and what each pixel is |
| `paint.frag` | the painting: reads the two small pictures and lays them down in brush strokes over the window, the strokes running along edges and contours, with finer strokes for the figure and the orbs; then the orbs' glow, and the compass and the count, drawn on top with distance fields |

The scene shader also draws the orbs (glowing spheres, id 4, hanging 1.7 m over the ground, with a halo, a thin column of light that reaches 26 m up so that they show from afar, and a pool of light on the ground around them).

## The game

Seven orbs of light lie in a wide ring (45 to 105 m) around where you start. Walk within 3 m of one to collect it. A compass in the top left corner points to the nearest orb that is left, and beside it is the count (`3/7`, seven-segment digits). When the last is collected the
clock turns the sky toward a golden dawn (the short way round, with `wrap` and `angle_diff`), the light turns gold, and every collected orb sends up a high column of light. The state remembers it (`world.done`).

- **From the seed.** `@start` seeds the `Rng` (`std/random.mito`, declared in the program's state as `rng: Rng`) with the world's seed and draws the places, so the same world always has the same orbs. The first one lies 42 m ahead of the start along the
  initial heading and 2.2 m to the side, so `world --hold w,shift --frames 110` walks into it, and it is not hidden behind the figure when you do.
- **The vector library and the math library.** The player's place and the nearest orb's are `Vec2`s in the state (`world.player.pos.x += ...`, `world.orbs.aim.set(x, z)`); the code uses `clamp`, `wrap` and `angle_diff` from `std/math.mito` where a float needs limiting or an angle needs the short way round.
  Orb coordinates are `f64[ORBS]` arrays, because a struct cannot be an array element in plain state yet.
- **What the shaders are told.** The frame's answer for `scene` carries the player, the clock, `dawn` (0 to 1), `taken` (which orbs are collected, as bits of a number) and `ox0 ... oz6`, the orbs' places; the line for `paint` carries the count, the total, the compass angle,
  the distance, `dawn` and the time. Only `uniform float`s exist, so seven orbs are fourteen names.
- **Tests.** `mito test examples/world/world.mito` runs three tests: the orbs lie in the ring and the first is ahead and in reach of a straight walk, none lie close to another, and collection is exactly the radius. A test cannot change the state, so the rules it checks are the
  pure functions `gap` and `in_reach` that `collect` uses and the state `@start` made. The ignored `game_renders_the_world` test also runs forward for 100, 104 and 125 frames and checks that the count in the corner changes only after the orb is reached.

## Painting changes

- **No halo around the figure.** Three causes were found and fixed: the world's strokes whose middle is on the figure took their color from straight outward (the sky above the head), which made long pale streaks; now they look to the sides. The figure's outline pulled the world's strokes
  into long vertical streaks around it, so the world's strokes ignore the form near the figure (the `calm` value). And the small picture's texels at the figure's edge mix figure with sky, which painted a pale rim: the figure takes the color of the nearest texel that really is the figure.
- **Seams between stroke grids.** The stroke angle is held in steps (a ninth of a half turn); where it is about to change, the grid of the next step is laid over the current one and fades in to half and half at the change, so two grids overlap there instead of meeting along a cut.
- **The head.** A round head, a mustard woolly hat with a pale rim and a pompom, dark hair at the back, and two dark eyes: it reads as a head from behind (the usual view) and from the front.

## What it shows

- **A game as an ordinary program.** `platform game;`, an `@frame` function whose inputs are what the window saw (`dt: f64 from time`, `forward: bool from key("w")`, `look_x: f64 from mouse("dx")`),
  and `@shader` functions that give the shaders' sources. The frame's answer is a line for each pass: `scene@0.25 px=330 pz=60 yaw=0.35 pitch=-0.03 day=0.10 seed=111` draws the world into an image a quarter of the window's size, and `paint` draws the window from it.
- **Nothing stored.** The ground exists everywhere because the shader computes it from noise and the seed: there is no map, no chunks and no edge, and the same place always looks the same.
  Change `world.seed` and the world is another one. The state is only the player, the orbs and the clock.
- **Painted, not photographed.** The picture is three layers of strokes, big under small, each a long narrow cell that follows a slowly turning flow field and bends like a wrist. A stroke is one flat
  color, the color of the world at the stroke's middle (so a stroke that crosses an edge takes one side), streaked along its length by bristles, a little darker where the paint thins at its edges, with warm paper
  showing through at the margins. Small distant detail dissolves into strokes: that is what broad brushwork does. The knobs are the stroke sizes, the flow's amount and the three `strokes(...)` calls in `main`.
- **Strokes follow the form.** The scene shader writes a second picture beside the light (`layout(location = 1) out vec4 aux;`, read by the paint pass as `uImage1`): depth, the normal's x and z, and an id (0 sky, 1 ground,
  2 water, 3 figure, 4 orb) in the fourth channel. The painter takes the gradient of that picture (Sobel, summed over three places around the pixel so the direction is smooth), and each stroke is turned across it, so strokes run along the shore, the horizon, hill
  silhouettes and the figure's outline. An angle is only known up to a half turn, so angles are blended as (cos 2a, sin 2a); where there is no edge the old flow field decides, and the angle is held in steps so strokes stay put.
- **A figure to follow.** The camera orbits a small walker (the mouse's yaw and pitch, 4.2 m away, never under the ground). `world.mito` turns the figure toward the way it travels, relative to the camera (`atan2`, `sin`, `cos`, the difference wrapped with
  `atan2(sin d, cos d)`), advances its stride with the distance walked, and passes `heading`, `walk` and `moving` as uniforms. The figure is id 3: it is painted with strokes 0.4 times the size of the world's, a stroke that is not on the figure
  takes its own pixel's color, and the world's strokes whose middle is on the figure take the color of the nearest place off it, so the figure's red does not smear.
- **Rules in the language, picture on the GPU.** Walking along the direction you face is `sin` and `cos` of the yaw; looking up and down is clamped with `max(-1.4, min(1.4, ...))` on floats.
- **Seeing it without a window.** `--frames N --screenshot f.ppm` renders in a window that is not shown and saves the last frame. `cargo test game_renders_the_world -- --ignored` (with `MITO_SDL3_DIR` set) builds this
  example, renders it and checks the picture.

Speed: the world is drawn once into an image a quarter of the window's size and the strokes read their colors from it, so the expensive part (a ray march for every pixel) runs on a sixteenth of the pixels. On an
integrated Intel Arc GPU a frame took about 1.2 ms at 1280x720 before the figure and the second picture; about 2.5-3.5 ms at 1280x720 after that, and about 3.1-3.4 ms with the orbs, the panel and the grid overlap and 5-8 ms at 1920x1080 (best of five, wall time of 120 frames with vsync off, on a shared GPU, so noisy; the edge-following costs most). Drawing the world at full size for every stroke layer
that covers a pixel took 28 ms and 50 ms at the first two sizes.

Not here: collision (the camera follows the height of the ground, or of the water, so you walk across lakes), sound, saving the player and the orbs.
