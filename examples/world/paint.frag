#version 330 core
// The painting: the world, drawn small by scene.frag into `uImage`, laid down in broad brush strokes, with the game's compass and count painted on top.
in vec2 vUv;
out vec4 color;

uniform vec2 uResolution;    // the window, in pixels (set by the driver)
uniform sampler2D uImage;    // the world, in light (set by the driver: what the `scene@0.25` pass drew); its alpha is the orbs' glow
uniform sampler2D uImage1;   // what the strokes follow, drawn beside it: depth (0..1), normal x and z (0..1), and what it is (0 sky, 1 ground, 2 water, 3 figure, 4 orb) over 4
uniform float count;         // orbs collected
uniform float total;         // orbs in the world
uniform float arrow;         // where the nearest orb lies relative to where you look, radians, positive to the right
uniform float near;          // how far it is, meters (0: none left)
uniform float dawn;          // 0..1: the finished game's golden dawn
uniform float time;          // seconds

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
// it covers takes the color of the world seen at the stroke's middle, so a stroke is one flat color, a little darker at its edges,
// streaked along its length by the bristles. Three layers of strokes, big under small, are laid one over another, each at its own
// angle; the angle follows a slowly changing flow field, so the strokes sweep around like a hand does.

vec3 tone(vec3 c) {
    c = c / (1.0 + c * 0.35);
    return pow(c, vec3(1.0 / 2.2));
}

// what the scene drew at a window position, exactly (no blending between neighboring texels): 0 sky, 1 ground, 2 water, 3 the figure, 4 an orb
float idAt(vec2 p) {
    ivec2 sz = textureSize(uImage1, 0);
    return floor(texelFetch(uImage1, clamp(ivec2(p / uResolution * vec2(sz)), ivec2(0), sz - 1), 0).a * 4.0 + 0.5);
}

// 1 where the (blended) id a4 is the id `id`, 0 elsewhere, soft between
float isId(float a4, float id) { return smoothstep(id - 0.65, id - 0.35, a4) * (1.0 - smoothstep(id + 0.35, id + 0.65, a4)); }

// The color of a thing's own pixels near a window position. The small picture is blown up with blending, so at the edge of the figure a texel mixes the figure with the sky behind it,
// and painted as it is that makes a pale fringe around the figure: take the color of the nearest texel that really is the thing instead.
vec3 thingColor(vec2 at, float id) {
    ivec2 sz = textureSize(uImage, 0);
    vec2 f = at / uResolution * vec2(sz);
    ivec2 c = clamp(ivec2(f), ivec2(0), sz - 1);
    vec3 best = texelFetch(uImage, c, 0).rgb;
    float bd = 1e9;
    for (int j = -1; j <= 1; j++) {
        for (int i = -1; i <= 1; i++) {
            ivec2 n = clamp(c + ivec2(i, j), ivec2(0), sz - 1);
            if (floor(texelFetch(uImage1, n, 0).a * 4.0 + 0.5) == id) {
                vec2 dv = vec2(n) + 0.5 - f;
                float d = dot(dv, dv);
                if (d < bd) { bd = d; best = texelFetch(uImage, n, 0).rgb; }
            }
        }
    }
    return best;
}

// Which way the form runs at a place, as a vector (cos 2a, sin 2a) of the stroke's angle a (an angle is only defined up to a half turn, so
// angles are averaged as these vectors); its length says how sure that is. The strokes run along the edges and contours of the scene: across where depth,
// the surface's tilt or what the thing is changes fastest. That is the gradient of the second image (Sobel, in units of a few texels), its tensor
// (gx*gx, gy*gy, gx*gy) summed over several places around the pixel (the blur that keeps the direction smooth), and the stroke runs across the gradient.
vec3 formTensor(vec2 uv, vec2 step) {
    const vec4 W = vec4(7.0, 1.6, 1.6, 3.2);             // how much an edge of depth, tilt and kind counts
    vec4 tl = texture(uImage1, uv + step * vec2(-1.0, 1.0)) * W, t = texture(uImage1, uv + step * vec2(0.0, 1.0)) * W, tr = texture(uImage1, uv + step * vec2(1.0, 1.0)) * W;
    vec4 l = texture(uImage1, uv + step * vec2(-1.0, 0.0)) * W, r = texture(uImage1, uv + step * vec2(1.0, 0.0)) * W;
    vec4 bl = texture(uImage1, uv + step * vec2(-1.0, -1.0)) * W, b = texture(uImage1, uv + step * vec2(0.0, -1.0)) * W, br = texture(uImage1, uv + step * vec2(1.0, -1.0)) * W;
    vec4 gx = (tr + 2.0 * r + br) - (tl + 2.0 * l + bl), gy = (tl + 2.0 * t + tr) - (bl + 2.0 * b + br);
    return vec3(dot(gx, gx), dot(gy, gy), dot(gx, gy));
}
vec3 formVector(vec2 pix, float radius) {
    vec2 uv = pix / uResolution, texel = 1.0 / vec2(textureSize(uImage1, 0)), h = texel * radius;
    vec3 T = formTensor(uv, h) * 2.0 + formTensor(uv + h * vec2(1.4, 1.1), h) + formTensor(uv - h * vec2(1.4, 1.1), h);
    // the gradient's doubled angle is (txx - tyy, 2 txy); a stroke is across it, which is half a turn on the doubled angle
    return vec3(-(T.x - T.y), -2.0 * T.z, 0.0) / 4.0;
}

// the color of a stroke, given the window position of its middle; a little paint's worth of variation from stroke to stroke.
// paint is more saturated than light, and stronger in its lights and darks
vec3 grade(vec3 light) {
    vec3 c = tone(light);
    float luma = dot(c, vec3(0.299, 0.587, 0.114));
    c = mix(vec3(luma), c, 1.3);
    return clamp((c - 0.5) * 1.22 + 0.5, 0.0, 1.0);
}

// Where a world stroke takes its color when its middle is on the figure or an orb: its flat color would smear the figure's red over the grass, so it takes the color of the nearest place off it,
// looking to the sides (the ground and the sky beside the figure, not the sky above its head, which would make pale streaks).
vec2 offThing(vec2 center) {
    float k = uResolution.y / 720.0;
    for (int i = 0; i < 12; i++) {
        float s = (8.0 + 8.0 * float(i)) * k;
        if (idAt(center + vec2(s, 0.0)) < 2.5) return center + vec2(s, 0.0);
        if (idAt(center - vec2(s, 0.0)) < 2.5) return center - vec2(s, 0.0);
    }
    return center;
}

// the color of a stroke, given the window position of its middle; a little paint's worth of variation from stroke to stroke.
// `only` is 0 for a stroke of the world, or the id of the thing (3 the figure, 4 an orb) a stroke belongs to: that takes its own pixel's color where its middle is not on the thing.
vec3 paint(vec2 center, vec2 pix, vec2 id, float layer, float only) {
    vec3 c;
    if (only > 0.5) {
        bool middleOn = abs(idAt(center) - only) < 0.5;
        c = grade(thingColor(middleOn ? center : pix, only));
    } else {
        vec2 at = center;
        if (idAt(center) > 2.5) at = offThing(center);
        c = grade(texture(uImage, clamp(at / uResolution, 0.001, 0.999)).rgb);
    }
    float v = hash(id * 1.37 + layer * 11.0);
    c *= 0.93 + 0.14 * v;                                // each stroke a touch lighter or darker
    c += (vec3(hash(id + 3.1), hash(id + 7.7), hash(id + 9.9)) - 0.5) * 0.045;   // and a touch of another hue
    return c;
}

// One grid of strokes at one angle: x is the color and y how much of this pixel the stroke covers (0 where the brush did not reach).
// `len` and `wid` are the stroke's size in pixels, `fill` 1 for a layer that covers everything.
vec4 grid(vec2 pix, float layer, float len, float wid, float fill, float ang, float only) {
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
    vec2 center = transpose(R) * mid;
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
    if (only > 0.5) cover *= isId(texture(uImage1, pix / uResolution).a * 4.0, only);   // a thing's own strokes stay on the thing
    if (cover <= 0.0) return vec4(0.0);
    vec3 c = paint(center, pix, id, layer, only);
    // bristles: fine streaks along the stroke, and the paint thinning toward the edges
    float bristle = noise(vec2(q.x * 0.035, q.y * 1.3) + id * 3.0) * 0.6 + noise(vec2(q.x * 0.1, q.y * 2.7) + id) * 0.4;
    c *= 0.86 + 0.28 * bristle;
    c = mix(c * 0.78, c, smoothstep(0.95, 0.55, shape));
    return vec4(c, cover);
}

const float STEP = 0.34906585;   // the stroke angle is held in steps of a ninth of a half turn

// One layer of strokes. `turn` is the layer's own angle added to the flow, `calm` (0..1) how much to ignore the form here (near the figure, whose outline would otherwise
// pull strokes of the world into long streaks around it). `only` as in paint().
vec4 strokes(vec2 pix, float layer, float len, float wid, float turn, float fill, float radius, float only, float calm) {
    // the flow field: a direction that changes slowly across the window, held in steps so that neighboring strokes agree
    float flow = (noise(pix * 0.0022 + layer * 5.0) - 0.5) * 1.7 + turn;
    // ... bent to run along the form where the second image has an edge or a contour (as doubled-angle vectors), then held in steps
    vec3 form = formVector(pix, radius);
    float sure = smoothstep(0.35, 1.4, length(form.xy)) * (1.0 - calm);
    vec2 dir = mix(vec2(cos(2.0 * flow), sin(2.0 * flow)), normalize(form.xy + 1e-6), sure);
    float u = 0.5 * atan(dir.y, dir.x) / STEP;
    float k0 = floor(u + 0.5), f = u - k0;
    vec4 a = grid(pix, layer, len, wid, fill, k0 * STEP, only);
    // Where the held angle is about to change to the next step, the grid of the next step is laid over this one, fading in to half and half at the change: the seam between two grids
    // becomes an overlap of strokes of both, not a straight cut.
    float w = smoothstep(0.40, 0.5, abs(f)) * 0.5;
    if (w > 0.002) {
        vec4 b = grid(pix, layer, len, wid, fill, (k0 + (f > 0.0 ? 1.0 : -1.0)) * STEP, only);
        float cover = mix(a.a, b.a, w);
        return vec4((a.rgb * a.a * (1.0 - w) + b.rgb * b.a * w) / max(cover, 1e-4), cover);
    }
    return a;
}

// ---- the game's panel, painted on top: a compass arrow pointing at the nearest orb, and how many are collected ----
float sdBox(vec2 p, vec2 b, float r) { vec2 d = abs(p) - b + r; return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r; }
float sdTriangle(vec2 p, vec2 p0, vec2 p1, vec2 p2) {
    vec2 e0 = p1 - p0, e1 = p2 - p1, e2 = p0 - p2, v0 = p - p0, v1 = p - p1, v2 = p - p2;
    vec2 q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0), q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0), q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    float s = sign(e0.x * e2.y - e0.y * e2.x);
    vec2 d = min(min(vec2(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)), vec2(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))), vec2(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)));
    return -sqrt(d.x) * sign(d.y);
}
float sdSegment(vec2 p, vec2 a, vec2 b) { vec2 pa = p - a, ba = b - a; return length(pa - ba * clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0)); }
// a seven-segment digit: s is the half size, p is measured from the digit's middle (y down); segments a (top) b c d e f (clockwise) g (middle) are bits 0..6
float digit(vec2 p, int n, vec2 s) {
    int m = int[10](63, 6, 91, 79, 102, 109, 125, 7, 127, 111)[clamp(n, 0, 9)];
    float t = s.x * 0.18, gx = s.x - t, gy = s.y - t, d = 1e9;
    if ((m & 1) != 0) d = min(d, sdSegment(p, vec2(-gx, -gy) * vec2(0.8, 1.0), vec2(gx, -gy) * vec2(0.8, 1.0)));
    if ((m & 8) != 0) d = min(d, sdSegment(p, vec2(-gx, gy) * vec2(0.8, 1.0), vec2(gx, gy) * vec2(0.8, 1.0)));
    if ((m & 64) != 0) d = min(d, sdSegment(p, vec2(-gx, 0.0) * 0.8, vec2(gx, 0.0) * 0.8));
    if ((m & 2) != 0) d = min(d, sdSegment(p, vec2(gx, -gy), vec2(gx, -t * 0.2)));
    if ((m & 4) != 0) d = min(d, sdSegment(p, vec2(gx, t * 0.2), vec2(gx, gy)));
    if ((m & 16) != 0) d = min(d, sdSegment(p, vec2(-gx, t * 0.2), vec2(-gx, gy)));
    if ((m & 32) != 0) d = min(d, sdSegment(p, vec2(-gx, -gy), vec2(-gx, -t * 0.2)));
    return d - t;
}
vec3 lay(vec3 col, vec3 c, float d) { return mix(col, c, 1.0 - smoothstep(-0.8, 0.8, d)); }   // a shape of color c where the distance d is negative
vec3 hud(vec3 col, vec2 pix, float k) {
    vec2 p = vec2(pix.x, uResolution.y - pix.y) / k - vec2(24.0, 24.0);   // from the panel's top left corner, y down, in the 720-pixel layout
    if (p.x > 214.0 || p.y > 100.0) return col;
    vec3 cream = vec3(0.98, 0.93, 0.78), ink = vec3(0.07, 0.05, 0.04);
    vec3 tint = mix(vec3(1.0, 0.45, 0.70), vec3(1.0, 0.80, 0.34), dawn);
    bool finished = count >= total - 0.5;
    p += (vec2(noise(p * 0.21), noise(p * 0.21 + 7.0)) - 0.5) * 1.6;      // the edges waver a little, as if painted
    // the panel: a dark patch of ink with a pale rim
    float pd = sdBox(p - vec2(98.0, 40.0), vec2(98.0, 40.0), 18.0);
    col = mix(col, ink, 0.66 * (1.0 - smoothstep(-1.0, 1.0, pd)));
    col = lay(col, cream * 0.75, abs(pd + 3.5) - 1.1);
    // the compass: a ring and an arrow turned toward the nearest orb; it glows brighter the nearer the orb is
    vec2 c = p - vec2(44.0, 40.0);
    col = lay(col, cream * 0.7, abs(length(c) - 28.0) - 1.4);
    if (!finished) {
        vec2 u = vec2(sin(arrow), -cos(arrow));
        vec2 l = vec2(dot(c, vec2(-u.y, u.x)), dot(c, u));   // x across the arrow, y along it
        float ad = min(sdTriangle(l, vec2(0.0, 22.0), vec2(-12.0, -14.0), vec2(0.0, -5.0)), sdTriangle(l, vec2(0.0, 22.0), vec2(12.0, -14.0), vec2(0.0, -5.0)));
        float close = smoothstep(60.0, 6.0, near);
        vec3 ac = mix(tint, vec3(1.0), 0.25 + 0.5 * close * (0.5 + 0.5 * sin(time * 8.0)));
        col = lay(col, ink, ad - 3.0);
        col = lay(col, ac, ad);
    } else {
        float pulse = 0.5 + 0.5 * sin(time * 3.0);
        col = lay(col, ink, length(c) - 15.0);
        col = lay(col, tint * (1.0 + 0.4 * pulse), length(c) - 11.0 - 2.0 * pulse);
    }
    // the orb icon, the count and the total: 3 / 7
    vec2 ic = p - vec2(94.0, 40.0);
    col = lay(col, tint * 0.45, length(ic) - 15.0 - 3.0 * (0.5 + 0.5 * sin(time * 2.0)));
    col = lay(col, tint * 1.1 + 0.2, length(ic) - 8.5);
    int n = int(count + 0.5), tot = int(total + 0.5);
    vec3 nc = finished ? mix(tint, vec3(1.0), 0.2 + 0.2 * sin(time * 3.0)) : cream;
    vec2 dp = p - vec2(130.0, 40.0);
    float dd = digit(dp, n, vec2(12.0, 21.0));
    col = lay(col, ink, dd - 2.5);
    col = lay(col, nc, dd);
    vec2 sp = p - vec2(153.0, 40.0);
    float sd = sdSegment(sp, vec2(4.0, -13.0), vec2(-4.0, 13.0)) - 1.6;
    col = lay(col, cream * 0.8, sd);
    dd = digit(p - vec2(176.0, 46.0), tot, vec2(8.0, 14.0));
    col = lay(col, ink, dd - 2.0);
    col = lay(col, cream * 0.8, dd);
    return col;
}

void main() {
    float k = uResolution.y / 720.0;                     // strokes keep their proportions at any window size
    vec2 pix = vUv * uResolution;
    // the canvas: warm paper, which shows through wherever the paint is thin
    vec3 paper = vec3(0.93, 0.89, 0.80) * (0.96 + 0.06 * noise(pix * 0.8));

    // how close this place is to the figure or an orb, looked up at two rings around the pixel: the world's strokes ignore the outline there
    float nearThing = 0.0;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982;
        vec2 o = vec2(cos(a), sin(a));
        nearThing += (idAt(pix + o * 18.0 * k) > 2.5 ? 1.0 : 0.0) + (idAt(pix + o * 40.0 * k) > 2.5 ? 1.0 : 0.0);
    }
    float calm = clamp(nearThing * 0.5, 0.0, 1.0);

    vec4 s0 = strokes(pix, 0.0, 170.0 * k, 58.0 * k, 0.0, 1.0, 3.0, 0.0, calm);     // the broad underpainting
    vec3 col = s0.rgb;
    vec4 s1 = strokes(pix, 1.0, 108.0 * k, 36.0 * k, 0.3, 0.0, 2.0, 0.0, calm);     // the middle strokes
    col = mix(col, s1.rgb, s1.a);
    vec4 s2 = strokes(pix, 2.0, 64.0 * k, 19.0 * k, -0.25, 0.0, 1.5, 0.0, calm);    // the small strokes that pick out detail
    col = mix(col, s2.rgb, s2.a * 0.95);
    // the figure, in finer strokes (0.4 of the world's) so that it keeps its detail; they only land where the scene drew the figure
    float a4 = texture(uImage1, pix / uResolution).a * 4.0;
    float fig = isId(a4, 3.0);
    if (fig > 0.0) {
        col = mix(col, grade(thingColor(pix, 3.0)), fig);   // under the figure's strokes: the figure itself, not the world's strokes
        vec4 s3 = strokes(pix, 3.0, 64.0 * 0.4 * k, 19.0 * 0.4 * k, 0.2, 0.0, 1.0, 3.0, 0.0);
        col = mix(col, s3.rgb, s3.a);
    }
    // the orbs, in small strokes too
    float orb = isId(a4, 4.0);
    if (orb > 0.0) {
        col = mix(col, grade(thingColor(pix, 4.0)), orb);
        vec4 s4 = strokes(pix, 4.0, 64.0 * 0.25 * k, 19.0 * 0.25 * k, 0.5, 0.0, 1.0, 4.0, 0.0);
        col = mix(col, s4.rgb, s4.a);
    }

    // the orbs' glow, laid over the paint as soft light (the scene keeps it in the alpha of its picture): a screen of the light's color, so it never clips
    float glow = texture(uImage, pix / uResolution).a * (0.9 + 0.2 * noise(pix * 0.05 + time * 0.3));
    vec3 gc = mix(vec3(1.0, 0.40, 0.68), vec3(1.0, 0.78, 0.32), dawn);
    col = mix(col, gc * (0.9 + 0.3 * min(glow, 2.0)), 0.8 * (1.0 - exp(-glow * 0.9)));

    // a little paper in the grain, and a soft darkening toward the corners
    col = mix(col, paper, 0.05 + 0.05 * noise(pix * 0.35));
    vec2 v = vUv - 0.5;
    col *= 1.0 - dot(v, v) * 0.45;
    col = hud(col, pix, k);
    color = vec4(col, 1.0);
}
