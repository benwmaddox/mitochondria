#version 330 core
// The painting: the world, drawn small by scene.frag into `uImage`, laid down in broad brush strokes.
in vec2 vUv;
out vec4 color;

uniform vec2 uResolution;    // the window, in pixels (set by the driver)
uniform sampler2D uImage;    // the world, in light (set by the driver: what the `scene@0.25` pass drew)

float hash(vec2 p) {
    p = fract((p + 1901.43) * vec2(0.1031, 0.1030));   // the offset keeps small coordinates well mixed
    p += dot(p, p.yx + 33.33);
    return fract((p.x + p.y) * p.x);
}

float noise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}

// ---- the painting ----
// The picture is laid down in broad brush strokes instead of being drawn pixel by pixel. A stroke is a long, narrow cell; every pixel
// it covers takes the colour of the world seen at the stroke's middle, so a stroke is one flat colour, a little darker at its edges,
// streaked along its length by the bristles. Three layers of strokes, big under small, are laid one over another, each at its own
// angle; the angle follows a slowly changing flow field, so the strokes sweep around like a hand does.

vec3 tone(vec3 c) {
    c = c / (1.0 + c * 0.35);
    return pow(c, vec3(1.0 / 2.2));
}

// the colour of a stroke, given the window position of its middle; a little paint's worth of variation from stroke to stroke
vec3 paint(vec2 centre, vec2 id, float layer) {
    vec3 c = tone(texture(uImage, clamp(centre / uResolution, 0.001, 0.999)).rgb);
    float v = hash(id * 1.37 + layer * 11.0);
    float luma = dot(c, vec3(0.299, 0.587, 0.114));
    c = mix(vec3(luma), c, 1.3);                         // paint is more saturated than light
    c = clamp((c - 0.5) * 1.22 + 0.5, 0.0, 1.0);          // and stronger in its lights and darks
    c *= 0.93 + 0.14 * v;                                // each stroke a touch lighter or darker
    c += (vec3(hash(id + 3.1), hash(id + 7.7), hash(id + 9.9)) - 0.5) * 0.045;   // and a touch of another hue
    return c;
}

// One layer of strokes: x is the colour and y how much of this pixel the stroke covers (0 where the brush did not reach).
// `len` and `wid` are the stroke's size in pixels, `turn` the layer's own angle added to the flow, `fill` 1 for a layer that covers everything.
vec4 strokes(vec2 pix, float layer, float len, float wid, float turn, float fill) {
    // the flow field: a direction that changes slowly across the window, held in steps so that neighbouring strokes agree
    float flow = (noise(pix * 0.0022 + layer * 5.0) - 0.5) * 1.7 + turn;
    float ang = floor(flow * 2.5 + 0.5) / 2.5;
    float ca = cos(ang), sa = sin(ang);
    mat2 R = mat2(ca, -sa, sa, ca);                      // into the stroke's own frame: x along it, y across
    vec2 q = R * pix;
    q.y += (noise(vec2(q.x * 0.004, q.y / (wid * 5.0)) + layer * 13.0) - 0.5) * wid * 2.4;   // the rows bend, as a wrist does
    float row = floor(q.y / wid);
    float stagger = hash(vec2(row, layer)) * len;
    float col = floor((q.x + stagger) / len);
    vec2 id = vec2(col, row) + layer * 31.0;
    vec2 local = vec2(fract((q.x + stagger) / len), fract(q.y / wid));
    // where the stroke's middle is, in the window
    vec2 mid = vec2((col + 0.5) * len - stagger, (row + 0.5) * wid) + (vec2(hash(id), hash(id + 5.0)) - 0.5) * vec2(len * 0.25, wid * 0.4);
    vec2 centre = transpose(R) * mid;
    // the stroke's shape: long, with a ragged edge and a rounded end
    float along = abs(local.x - 0.5) * 2.0 / (0.55 + 0.4 * hash(id + 2.2)), across = abs(local.y - 0.5) * 2.0;
    float ragged = (noise(vec2(q.x * 0.09, q.y * 0.5) + id) - 0.5) * 0.5;
    float shape = max(pow(along, 3.0), across * 0.98) + ragged;
    // the taper: toward its tip a stroke narrows, then breaks into a few thin strands (the bristles that still carry paint), and ends
    float tail = smoothstep(0.45, 1.0, along);                       // 0 along the body, 1 at the tip
    float strands = noise(vec2(q.y * 0.8, id.x * 3.1 + layer * 7.0)) * 0.65 + noise(vec2(q.y * 2.1, id.y + layer)) * 0.35;
    float body = 1.0 - smoothstep(0.72, 0.95, across * (1.0 + tail * 0.55) + ragged);
    float bristles = smoothstep(tail * 0.88 - 0.1, tail * 0.88, strands);
    float tip = 1.0 - smoothstep(1.0, 1.3, along + ragged * 0.4);
    float cover = fill > 0.5 ? 1.0 : body * bristles * tip;
    if (cover <= 0.0) return vec4(0.0);
    vec3 c = paint(centre, id, layer);
    // bristles: fine streaks along the stroke, and the paint thinning toward the edges
    float bristle = noise(vec2(q.x * 0.035, q.y * 1.3) + id * 3.0) * 0.6 + noise(vec2(q.x * 0.1, q.y * 2.7) + id) * 0.4;
    c *= 0.86 + 0.28 * bristle;
    c = mix(c * 0.78, c, smoothstep(0.95, 0.55, shape));
    return vec4(c, cover);
}

void main() {
    float k = uResolution.y / 720.0;                     // strokes keep their proportions at any window size
    vec2 pix = vUv * uResolution;
    // the canvas: warm paper, which shows through wherever the paint is thin
    vec3 paper = vec3(0.93, 0.89, 0.80) * (0.96 + 0.06 * noise(pix * 0.8));

    vec4 s0 = strokes(pix, 0.0, 170.0 * k, 58.0 * k, 0.0, 1.0);     // the broad underpainting
    vec3 col = s0.rgb;
    vec4 s1 = strokes(pix, 1.0, 108.0 * k, 36.0 * k, 0.3, 0.0);     // the middle strokes
    col = mix(col, s1.rgb, s1.a);
    vec4 s2 = strokes(pix, 2.0, 64.0 * k, 19.0 * k, -0.25, 0.0);    // the small strokes that pick out detail
    col = mix(col, s2.rgb, s2.a * 0.95);

    // a little paper in the grain, and a soft darkening toward the corners
    col = mix(col, paper, 0.05 + 0.05 * noise(pix * 0.35));
    vec2 v = vUv - 0.5;
    col *= 1.0 - dot(v, v) * 0.45;
    color = vec4(col, 1.0);
}
