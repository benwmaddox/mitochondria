#version 330 core
// The painting: the world, drawn small by scene.frag into `uImage`, laid down in broad brush strokes.
in vec2 vUv;
out vec4 color;

uniform vec2 uResolution;    // the window, in pixels (set by the driver)
uniform sampler2D uImage;    // the world, in light (set by the driver: what the `scene@0.25` pass drew)
uniform sampler2D uImage1;   // what the strokes follow, drawn beside it: depth (0..1), normal x and z (0..1), and what it is (0 sky, 1 ground, 2 water, 3 figure) over 4

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

// what the scene drew at a window position, exactly (no blending between neighbouring texels): 0 sky, 1 ground, 2 water, 3 the figure
float idAt(vec2 p) {
    ivec2 sz = textureSize(uImage1, 0);
    return floor(texelFetch(uImage1, clamp(ivec2(p / uResolution * vec2(sz)), ivec2(0), sz - 1), 0).a * 4.0 + 0.5);
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

// the colour of a stroke, given the window position of its middle; a little paint's worth of variation from stroke to stroke.
// paint is more saturated than light, and stronger in its lights and darks
vec3 grade(vec3 light) {
    vec3 c = tone(light);
    float luma = dot(c, vec3(0.299, 0.587, 0.114));
    c = mix(vec3(luma), c, 1.3);
    return clamp((c - 0.5) * 1.22 + 0.5, 0.0, 1.0);
}

// Where a world stroke takes its colour when its middle is on the figure: its flat colour would smear the figure's red over the grass, so it takes the
// colour of the nearest place off the figure, going outward from the middle of the window (the camera looks at the figure, so that is where the figure is).
vec2 offFigure(vec2 centre) {
    vec2 away = normalize(centre - uResolution * 0.5 + vec2(0.01, 0.02));
    vec2 p = centre;
    for (int i = 0; i < 10; i++) {
        if (idAt(p) < 2.5) break;
        p += away * (10.0 + 4.0 * float(i)) * uResolution.y / 720.0;
    }
    return p;
}

// the colour of a stroke, given the window position of its middle; a little paint's worth of variation from stroke to stroke.
// A figure stroke whose middle is off the figure takes its own pixel's colour instead.
vec3 paint(vec2 centre, vec2 pix, vec2 id, float layer, bool ofFigure) {
    bool middleOnFigure = idAt(centre) > 2.5;
    vec2 at = centre;
    if (ofFigure && !middleOnFigure) at = pix;
    else if (!ofFigure && middleOnFigure) at = offFigure(centre);
    vec3 c = grade(texture(uImage, clamp(at / uResolution, 0.001, 0.999)).rgb);
    float v = hash(id * 1.37 + layer * 11.0);
    c *= 0.93 + 0.14 * v;                                // each stroke a touch lighter or darker
    c += (vec3(hash(id + 3.1), hash(id + 7.7), hash(id + 9.9)) - 0.5) * 0.045;   // and a touch of another hue
    return c;
}

// One layer of strokes: x is the colour and y how much of this pixel the stroke covers (0 where the brush did not reach).
// `len` and `wid` are the stroke's size in pixels, `turn` the layer's own angle added to the flow, `fill` 1 for a layer that covers everything.
vec4 strokes(vec2 pix, float layer, float len, float wid, float turn, float fill, float radius, bool ofFigure) {
    // the flow field: a direction that changes slowly across the window, held in steps so that neighbouring strokes agree
    float flow = (noise(pix * 0.0022 + layer * 5.0) - 0.5) * 1.7 + turn;
    // ... bent to run along the form where the second image has an edge or a contour (as doubled-angle vectors), then held in steps
    vec3 form = formVector(pix, radius);
    float sure = smoothstep(0.35, 1.4, length(form.xy));
    vec2 dir = mix(vec2(cos(2.0 * flow), sin(2.0 * flow)), normalize(form.xy + 1e-6), sure);
    float ang = floor(0.5 * atan(dir.y, dir.x) * 3.0 + 0.5) / 3.0;
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
    if (ofFigure) cover *= smoothstep(2.35, 2.65, texture(uImage1, pix / uResolution).a * 4.0);   // the figure's own strokes stay on the figure
    if (cover <= 0.0) return vec4(0.0);
    vec3 c = paint(centre, pix, id, layer, ofFigure);
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

    vec4 s0 = strokes(pix, 0.0, 170.0 * k, 58.0 * k, 0.0, 1.0, 3.0, false);     // the broad underpainting
    vec3 col = s0.rgb;
    vec4 s1 = strokes(pix, 1.0, 108.0 * k, 36.0 * k, 0.3, 0.0, 2.0, false);     // the middle strokes
    col = mix(col, s1.rgb, s1.a);
    vec4 s2 = strokes(pix, 2.0, 64.0 * k, 19.0 * k, -0.25, 0.0, 1.5, false);    // the small strokes that pick out detail
    col = mix(col, s2.rgb, s2.a * 0.95);
    // the figure, in finer strokes (0.4 of the world's) so that it keeps its detail; they only land where the scene drew the figure
    float fig = smoothstep(2.35, 2.65, texture(uImage1, pix / uResolution).a * 4.0);
    if (fig > 0.0) {
        col = mix(col, grade(texture(uImage, pix / uResolution).rgb), fig);   // under the figure's strokes: the figure itself, not the world's strokes
        vec4 s3 = strokes(pix, 3.0, 64.0 * 0.4 * k, 19.0 * 0.4 * k, 0.2, 0.0, 1.0, true);
        col = mix(col, s3.rgb, s3.a);
    }

    // a little paper in the grain, and a soft darkening toward the corners
    col = mix(col, paper, 0.05 + 0.05 * noise(pix * 0.35));
    vec2 v = vUv - 0.5;
    col *= 1.0 - dot(v, v) * 0.45;
    color = vec4(col, 1.0);
}
