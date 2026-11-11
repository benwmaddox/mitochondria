#version 330 core
// The world as a camera sees it, drawn small: ground, lakes, sky, sun and fog, made from the seed and where the camera is. The frame draws this into an image a
// fraction of the window's size (`scene@0.25`) and paint.frag lays it down in brush strokes.
// For each pixel a ray leaves the camera and is marched over a height field until it meets the ground; the height field is
// fractal value noise, so it exists everywhere and needs no stored map.
in vec2 vUv;
out vec4 color;

uniform vec2 uResolution;   // the window, in pixels (set by the driver)
uniform float px, pz;       // where the player stands
uniform float yaw, pitch;   // where the player looks
uniform float day;          // time of day, 0..1: 0 sunrise, 0.25 noon, 0.5 sunset
uniform float seed;         // which world

const float WATER = 0.0;    // the height of the lakes

float hash(vec2 p) {
    p = fract((p + seed * 17.13) * vec2(0.1031, 0.1030));
    p += dot(p, p.yx + 33.33);
    return fract((p.x + p.y) * p.x);
}

float noise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}

// the ground's height at a place
float height(vec2 p) {
    float h = 0.0, a = 0.55, f = 0.0030;
    for (int i = 0; i < 5; i++) {
        h += a * noise(p * f);
        f *= 2.1; a *= 0.48;
        p += vec2(31.7, 17.3);
    }
    float hills = (h - 0.46) * 90.0;
    float peaks = max(h - 0.60, 0.0);
    return hills + peaks * peaks * 1400.0;
}

vec3 sunDirection() {
    float a = day * 6.2831853;                  // 0 at sunrise
    return normalize(vec3(cos(a) * 0.75, sin(a), 0.45));
}

vec3 skyColor(vec3 rd, vec3 sun) {
    float up = max(rd.y, 0.0);
    float daylight = smoothstep(-0.12, 0.18, sun.y);
    vec3 horizon = mix(vec3(0.04, 0.05, 0.09), vec3(0.72, 0.80, 0.90), daylight);
    vec3 zenith = mix(vec3(0.01, 0.02, 0.05), vec3(0.10, 0.30, 0.72), daylight);
    vec3 sky = mix(horizon, zenith, pow(up, 0.55));
    // the warm band when the sun is near the horizon
    float glow = pow(max(dot(rd, sun), 0.0), 6.0) * (1.0 - abs(sun.y)) * 0.6;
    sky += vec3(1.0, 0.45, 0.2) * glow * smoothstep(-0.2, 0.2, sun.y);
    float disc = pow(max(dot(rd, sun), 0.0), 900.0) * 6.0 + pow(max(dot(rd, sun), 0.0), 40.0) * 0.4;
    return sky + vec3(1.0, 0.9, 0.7) * disc * smoothstep(-0.05, 0.05, sun.y);
}

vec3 groundNormal(vec2 p, float t) {
    float e = max(0.6, t * 0.004);
    return normalize(vec3(height(p - vec2(e, 0.0)) - height(p + vec2(e, 0.0)), 2.0 * e, height(p - vec2(0.0, e)) - height(p + vec2(0.0, e))));
}

// is the point in the shadow of the ground, looking toward the sun?
float shadow(vec3 p, vec3 sun) {
    float lit = 1.0, t = 2.0;
    for (int i = 0; i < 10; i++) {
        vec3 q = p + sun * t;
        float d = q.y - max(height(q.xz), WATER);
        lit = min(lit, 10.0 * d / t);
        t += max(d, 3.0 + t * 0.3);
        if (lit < 0.0 || t > 260.0) break;
    }
    return clamp(lit, 0.0, 1.0);
}

void main() {
    vec2 q = (vUv * 2.0 - 1.0) * vec2(uResolution.x / uResolution.y, 1.0);

    // the camera: standing on the ground (or on the water), looking where yaw and pitch say
    float ground = max(height(vec2(px, pz)), WATER);
    vec3 ro = vec3(px, ground + 1.7, pz);
    vec3 f = vec3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch));
    vec3 r = vec3(cos(yaw), 0.0, -sin(yaw));
    vec3 u = cross(f, r);
    vec3 rd = normalize(f + (q.x * r + q.y * u) * 0.75);

    vec3 sun = sunDirection();
    float daylight = smoothstep(-0.12, 0.18, sun.y);
    vec3 sky = skyColor(rd, sun);
    vec3 col = sky;

    // march the ray over the ground
    const float FAR = 900.0;
    float t = 0.5;
    bool hit = false;
    for (int i = 0; i < 90; i++) {
        vec3 p = ro + rd * t;
        float d = p.y - height(p.xz);
        if (d < 0.0015 * t) { hit = true; break; }
        t += max(d * 0.6, 0.08 + t * 0.004);
        if (t > FAR) break;
    }

    // the lake is a plane: a ray that goes down meets it, unless ground comes first
    float tw = rd.y < -0.0001 ? (WATER - ro.y) / rd.y : FAR * 2.0;
    bool water = rd.y < -0.0001 && tw > 0.0 && (!hit || tw < t) && tw < FAR;

    if (water) {
        vec3 p = ro + rd * tw;
        vec2 w = p.xz * 0.35 + vec2(day * 40.0, 0.0);
        vec3 n = normalize(vec3((noise(w) - 0.5) * 0.12, 1.0, (noise(w + 9.0) - 0.5) * 0.12));
        float fres = pow(1.0 - max(dot(-rd, n), 0.0), 4.0);
        float depth = clamp((WATER - height(p.xz)) * 0.2, 0.0, 1.0);
        vec3 deep = mix(vec3(0.04, 0.22, 0.30), vec3(0.01, 0.06, 0.12), depth) * (0.25 + 0.75 * daylight);
        vec3 refl = skyColor(reflect(rd, n), sun);
        col = mix(deep, refl, 0.25 + 0.75 * fres);
        t = tw;
    } else if (hit) {
        vec3 p = ro + rd * t;
        vec3 n = groundNormal(p.xz, t);
        float h = p.y;
        float slope = 1.0 - n.y;
        // what the ground is made of: sand by the water, grass, rock on steep places, snow high up
        // lush green and dry gold meadow, in patches a few hundred metres across, with finer variation inside them
        vec3 grass = mix(vec3(0.10, 0.27, 0.07), vec3(0.34, 0.36, 0.09), smoothstep(0.35, 0.7, noise(p.xz * 0.006 + 40.0)));
        grass = mix(grass, grass * vec3(1.25, 1.1, 0.8), noise(p.xz * 0.15));
        vec3 rock = mix(vec3(0.33, 0.30, 0.27), vec3(0.50, 0.46, 0.40), noise(p.xz * 0.4));
        vec3 sand = vec3(0.70, 0.62, 0.42);
        vec3 snow = vec3(0.92, 0.94, 0.98);
        vec3 base = mix(sand, grass, smoothstep(0.6, 3.0, h));
        base = mix(base, rock, smoothstep(0.18, 0.38, slope));
        base = mix(base, snow, smoothstep(58.0, 72.0, h + noise(p.xz * 0.05) * 14.0) * smoothstep(0.55, 0.15, slope));
        float diff = max(dot(n, sun), 0.0) * shadow(p + n * 0.5, sun);
        vec3 ambient = skyColor(n, sun) * 0.32;
        col = base * (vec3(1.0, 0.93, 0.80) * diff * 1.5 * daylight + ambient + vec3(0.02, 0.025, 0.04));
    }

    // distance fades into the air
    float fog = 1.0 - exp(-t * 0.0011);
    col = mix(col, skyColor(vec3(rd.x, max(rd.y, 0.0) * 0.4, rd.z), sun), fog);

    color = vec4(col, 1.0);   // light, not yet tone mapped: the painting does that
}
