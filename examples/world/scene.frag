#version 330 core
// The world as a camera sees it, drawn small: ground, lakes, sky, sun and fog, made from the seed and where the camera is. The frame draws this into an image a
// fraction of the window's size (`scene@0.25`) and paint.frag lays it down in brush strokes.
// For each pixel a ray leaves the camera and is marched over a height field until it meets the ground; the height field is
// fractal value noise, so it exists everywhere and needs no stored map.
in vec2 vUv;
layout(location = 0) out vec4 color;   // the world, in light
layout(location = 1) out vec4 aux;     // what the strokes follow: depth, normal (x, z) and what it is (0 sky, 1 ground, 2 water, 3 figure) over 4

uniform vec2 uResolution;   // the window, in pixels (set by the driver)
uniform float px, pz;       // where the player stands
uniform float yaw, pitch;   // where the player looks
uniform float day;          // time of day, 0..1: 0 sunrise, 0.25 noon, 0.5 sunset
uniform float seed;         // which world
uniform float heading;      // the way the figure faces (radians; 0 faces +z)
uniform float walk;         // its stride phase, in radians
uniform float moving;       // 1 while it walks, 0 standing

uniform float time;         // seconds since the start: the orbs bob and pulse with it
uniform float dawn;         // 0..1: the golden dawn of the finished game
uniform float taken;        // which orbs are collected, as bits (bit i set: orb i is gone)
uniform float ox0, ox1, ox2, ox3, ox4, ox5, ox6;   // where the orbs hang (world x and z)
uniform float oz0, oz1, oz2, oz3, oz4, oz5, oz6;

const float WATER = 0.0;    // the height of the lakes
const int ORBS = 7;
const float ORB_R = 0.45;   // an orb's radius in meters

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
    sky += vec3(1.0, 0.9, 0.7) * disc * smoothstep(-0.05, 0.05, sun.y);
    return mix(sky, sky * vec3(1.18, 0.92, 0.62) + vec3(0.06, 0.03, 0.0), dawn * 0.85);   // the finished game's golden light
}

// ---- the orbs: glowing spheres that hang a little above the ground, with a soft halo and a thin column of light so that they show from afar ----
vec3 orbTint() { return mix(vec3(1.0, 0.42, 0.68), vec3(1.0, 0.80, 0.34), dawn); }
bool orbGone(int i) { return ((int(taken + 0.5) >> i) & 1) == 1; }
vec3 orbC[ORBS];   // the orbs' centers, found once per pixel (each needs the ground's height)
vec3 orbCenter(int i) { return orbC[i]; }
vec3 findOrb(int i) {
    vec2 g = vec2(0.0);
    if (i == 0) g = vec2(ox0, oz0); else if (i == 1) g = vec2(ox1, oz1); else if (i == 2) g = vec2(ox2, oz2); else if (i == 3) g = vec2(ox3, oz3);
    else if (i == 4) g = vec2(ox4, oz4); else if (i == 5) g = vec2(ox5, oz5); else g = vec2(ox6, oz6);
    return vec3(g.x, max(height(g), WATER) + 1.7 + 0.14 * sin(time * 1.7 + float(i) * 2.1), g.y);
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


// ---- the figure: a small walker made of signed-distance shapes, drawn only where the ray meets a sphere around it ----
float capsule(vec3 p, vec3 a, vec3 b, float r) {
    vec3 pa = p - a, ba = b - a;
    return length(pa - ba * clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0)) - r;
}
const vec3 SHIRT = vec3(0.75, 0.12, 0.10), TROUSERS = vec3(0.10, 0.16, 0.42), SKIN = vec3(0.85, 0.60, 0.45), HAIR = vec3(0.12, 0.07, 0.04), SHOES = vec3(0.08, 0.06, 0.05), HAT = vec3(0.95, 0.70, 0.10), HAT_BAND = vec3(0.98, 0.93, 0.78);
vec3 figureColor;
// p is relative to the figure's feet, already turned to face +z; its x is the figure's right
float figure(vec3 p) {
    float sw = sin(walk) * 0.7 * moving;
    float bob = abs(cos(walk)) * 0.035 * moving;
    p.y -= bob;
    float lean = 0.06 * moving;
    // legs swing about the hips
    vec3 hipL = vec3(-0.09, 0.62, 0.0), hipR = vec3(0.09, 0.62, 0.0);
    vec3 footL = hipL + vec3(0.0, -0.52 * cos(sw), 0.52 * sin(sw));
    vec3 footR = hipR + vec3(0.0, -0.52 * cos(-sw), 0.52 * sin(-sw));
    float legs = min(capsule(p, hipL, footL, 0.075), capsule(p, hipR, footR, 0.075));
    float shoes = min(capsule(p, footL, footL + vec3(0.0, 0.0, 0.10), 0.08), capsule(p, footR, footR + vec3(0.0, 0.0, 0.10), 0.08));
    // torso and head
    vec3 pl = p; pl.z -= (p.y - 0.62) * lean;
    float torso = capsule(pl, vec3(0.0, 0.70, 0.0), vec3(0.0, 0.98, 0.0), 0.17);
    float head = min(length(pl - vec3(0.0, 1.20, 0.015)) - 0.125, length(pl - vec3(0.0, 1.345, 0.0)) - 0.05);   // a round head with a woolly hat's pompom
    // arms swing against the legs
    vec3 shL = vec3(-0.21, 0.96, 0.0), shR = vec3(0.21, 0.96, 0.0);
    vec3 handL = shL + vec3(-0.03, -0.42 * cos(-sw), 0.42 * sin(-sw));
    vec3 handR = shR + vec3(0.03, -0.42 * cos(sw), 0.42 * sin(sw));
    float arms = min(capsule(pl, shL, handL, 0.055), capsule(pl, shR, handR, 0.055));
    float d = min(min(legs, shoes), min(torso, head));
    d = min(d, arms);
    // which part is nearest decides the color
    figureColor = SHIRT;
    float best = torso;
    if (legs < best) { best = legs; figureColor = TROUSERS; }
    if (shoes < best) { best = shoes; figureColor = SHOES; }
    if (head < best) {
        // a mustard hat over the top (its rim slants down toward the front), a paler band on the rim, skin below it, and two dark eyes on the face
        best = head;
        float rim = 1.215 - 0.11 * pl.z;
        figureColor = SKIN;
        if (pl.z > 0.07 && length(vec2(abs(pl.x) - 0.05, pl.y - 1.185)) < 0.026) figureColor = vec3(0.05, 0.04, 0.04);
        if (pl.z < -0.02) figureColor = HAIR;
        if (pl.y > rim) figureColor = pl.y < rim + 0.03 ? HAT_BAND : HAT;
    }
    if (arms < best) { best = arms; figureColor = pl.y > 0.80 ? SHIRT : SKIN; }
    return d;
}
const float FS = 1.35;   // the figure's size over its modelled 1.3 m
vec3 toFigure(vec3 w, vec3 feet) {
    vec3 d = (w - feet) / FS;
    float c = cos(heading), s = sin(heading);
    return vec3(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}
vec3 fromFigureDir(vec3 v) {
    float c = cos(heading), s = sin(heading);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

void main() {
    vec2 q = (vUv * 2.0 - 1.0) * vec2(uResolution.x / uResolution.y, 1.0);

    // the camera orbits the figure: behind and above it, looking at it, and never below the ground
    float ground = max(height(vec2(px, pz)), WATER);
    vec3 feet = vec3(px, ground, pz);
    vec3 target = feet + vec3(0.0, 1.15, 0.0);
    vec3 look = vec3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch));
    vec3 ro = target - look * 4.2;
    ro.y = max(ro.y, max(height(ro.xz), WATER) + 0.35);
    vec3 f = normalize(target - ro);
    vec3 r = normalize(vec3(f.z, 0.0, -f.x));
    vec3 u = cross(f, r);
    vec3 rd = normalize(f + (q.x * r + q.y * u) * 0.75);

    vec3 sun = sunDirection();
    for (int i = 0; i < ORBS; i++) orbC[i] = findOrb(i);
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

    float matId = 0.0;
    vec3 nrm = vec3(0.0, 1.0, 0.0);
    if (water) {
        matId = 2.0;
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
        matId = 1.0;
        vec3 p = ro + rd * t;
        vec3 n = groundNormal(p.xz, t);
        nrm = n;
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

    // the figure's soft blob shadow on the ground or the water, and the figure itself where the ray meets it
    if (matId > 0.5) {
        vec3 p = ro + rd * t;
        vec2 off = (p.xz - feet.xz) + sun.xz / max(sun.y, 0.25) * 0.4;
        col *= 1.0 - 0.5 * exp(-dot(off, off) / 0.28) * smoothstep(-0.05, 0.2, sun.y) * smoothstep(1.5, 0.0, abs(p.y - ground));
        // each orb lights the ground around it
        for (int i = 0; i < ORBS; i++) {
            if (orbGone(i)) continue;
            vec3 dv = p - orbCenter(i);
            float r2 = dot(dv, dv);
            col += orbTint() * (2.0 / (1.0 + r2 * 0.35)) * (0.35 + 0.65 * max(dot(nrm, -dv * inversesqrt(r2 + 0.01)), 0.0)) * (0.85 + 0.15 * sin(time * 2.0 + float(i)));
        }
    }
    vec3 bc = feet + vec3(0.0, 0.65 * FS, 0.0);
    vec3 oc = ro - bc;
    float bb = dot(oc, rd), disc = bb * bb - (dot(oc, oc) - 0.9 * FS * 0.9 * FS);
    if (disc > 0.0) {
        float tc = max(-bb - sqrt(disc), 0.0), tend = -bb + sqrt(disc);
        for (int i = 0; i < 28; i++) {
            vec3 pf = toFigure(ro + rd * tc, feet);
            float d = figure(pf) * FS;
            if (d < 0.002) {
                if (tc < t) {
                    vec3 body = figureColor;
                    float e = 0.01;
                    vec3 nl = normalize(vec3(figure(pf + vec3(e, 0.0, 0.0)) - figure(pf - vec3(e, 0.0, 0.0)), figure(pf + vec3(0.0, e, 0.0)) - figure(pf - vec3(0.0, e, 0.0)), figure(pf + vec3(0.0, 0.0, e)) - figure(pf - vec3(0.0, 0.0, e))));
                    nrm = fromFigureDir(nl);
                    float diff = max(dot(nrm, sun), 0.0);
                    float under = clamp(1.0 - pf.y / 0.6, 0.0, 1.0);   // the feet are in the grass's shade
                    col = body * (vec3(1.0, 0.93, 0.80) * diff * 1.5 * daylight + skyColor(nrm, sun) * 0.35 + vec3(0.03, 0.035, 0.05)) * (1.0 - 0.3 * under);
                    t = tc;
                    matId = 3.0;
                }
                break;
            }
            tc += d * 0.9;
            if (tc > tend || tc > t) break;
        }
    }

    // the orbs themselves: a bright sphere where the ray meets one in front of everything else
    for (int i = 0; i < ORBS; i++) {
        if (orbGone(i)) continue;
        vec3 oc = orbCenter(i) - ro;
        float tp = dot(oc, rd), d2 = dot(oc, oc) - tp * tp;
        if (tp > 0.0 && d2 < ORB_R * ORB_R) {
            float th = tp - sqrt(ORB_R * ORB_R - d2);
            if (th < t) {
                vec3 nn = normalize(rd * th - oc);
                float core = pow(max(dot(nn, -rd), 0.0), 2.0);
                col = orbTint() * (1.2 + 3.0 * core) + vec3(0.6 * core * core);
                nrm = nn;
                t = th;
                matId = 4.0;
            }
        }
    }

    // distance fades into the air
    float fog = 1.0 - exp(-t * 0.0011);
    col = mix(col, skyColor(vec3(rd.x, max(rd.y, 0.0) * 0.4, rd.z), sun), fog);
    col = mix(col, col * vec3(1.12, 0.98, 0.78) + vec3(0.04, 0.02, 0.0), dawn * 0.8);

    // the orbs' light: a halo around each and a thin column of it reaching up, added to the picture as glow (the alpha channel), which the painting lays over its strokes
    // so that it stays soft and visible from afar. When every orb is collected the columns of the collected ones rise high and bright (the dawn).
    float glow = 0.0;
    float tgone = t;   // the ground or the figure in front
    for (int i = 0; i < ORBS; i++) {
        bool gone = orbGone(i);
        if (gone && dawn < 0.01) continue;
        vec3 c = orbCenter(i);
        vec3 oc = c - ro;
        float tp = dot(oc, rd);
        if (tp < 0.5) continue;
        float pulse = 0.8 + 0.2 * sin(time * 2.0 + float(i));
        if (!gone && tp < tgone + 1.0) {
            float d2 = dot(oc, oc) - tp * tp;
            float s = 0.45 + 0.045 * tp;
            glow += exp(-d2 / (s * s)) * (1.4 * pulse);
        }
        // the column
        vec2 oh = oc.xz, dh = rd.xz;
        float a = dot(dh, dh);
        if (a > 1e-6) {
            float tb = dot(oh, dh) / a;
            vec2 dd = oh - dh * tb;
            float rel = ro.y + rd.y * tb - (c.y - 1.7);
            float colH = gone ? 40.0 + 160.0 * dawn : 26.0;
            float sb = (gone ? 2.0 : 0.6) + 0.03 * tb;
            if (tb > 0.5 && tb < tgone + 1.0 && rel > -1.0) {
                float fade = smoothstep(-1.0, 2.0, rel) * (1.0 - smoothstep(0.0, colH, rel));
                glow += exp(-dot(dd, dd) / (sb * sb)) * fade * (gone ? 1.4 * dawn : 1.0 * pulse);
            }
        }
    }

    color = vec4(col, glow);
    aux = vec4(1.0 - 1.0 / (1.0 + min(t, 3000.0) * 0.02), nrm.x * 0.5 + 0.5, nrm.z * 0.5 + 0.5, matId * 0.25);   // light, not yet tone mapped: the painting does that
}
