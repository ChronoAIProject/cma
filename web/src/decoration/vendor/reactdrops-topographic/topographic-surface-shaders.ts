/*!
 * Copyright (c) 2026 ReactDrops. ReactDrops Component License 1.0.
 * Source: crisjavr/reactdrops @ aed0014649a2f0964041ac33c5f02dc3a40c5e13.
 * See LICENSE, NOTICE and source-map.json in this directory.
 */

export const vertex = `#version 300 es
layout(location=0) in vec2 aPosition;
void main() { gl_Position = vec4(aPosition, 0., 1.); }`;
export const fragment = `#version 300 es
precision highp float;
precision highp sampler2D;
uniform sampler2D uHeight;
uniform vec2 uGrid, uExtent;
uniform float uMargin, uRatio, uCosine, uSine;
uniform vec3 uLineColor, uAccentColor;
out vec4 color;
vec4 weights(float t) {
  float t2=t*t, t3=t2*t;
  return vec4(1.-3.*t+3.*t2-t3, 4.-6.*t2+3.*t3, 1.+3.*t+3.*t2-3.*t3, t3)/6.;
}
// Four hardware bilinear taps evaluate a tensor cubic B-spline exactly.
vec3 surface(vec2 uv) {
  vec2 p=uv*(uGrid-1.), cell=floor(p), f=fract(p);
  vec4 x=weights(f.x), y=weights(f.y);
  vec2 gx=vec2(x.x+x.y,x.z+x.w), gy=vec2(y.x+y.y,y.z+y.w);
  vec2 px=cell.x+vec2(-1.+x.y/gx.x,1.+x.w/gx.y);
  vec2 py=cell.y+vec2(-1.+y.y/gy.x,1.+y.w/gy.y);
  vec3 a=texture(uHeight,(vec2(px.x,py.x)+.5)/uGrid).rgb;
  vec3 b=texture(uHeight,(vec2(px.y,py.x)+.5)/uGrid).rgb;
  vec3 c=texture(uHeight,(vec2(px.x,py.y)+.5)/uGrid).rgb;
  vec3 d=texture(uHeight,(vec2(px.y,py.y)+.5)/uGrid).rgb;
  return mix(mix(a,b,gx.y),mix(c,d,gx.y),gy.y);
}
void main() {
  vec2 pixels=vec2(gl_FragCoord.x/uRatio, (uExtent.y-2.*uMargin)-gl_FragCoord.y/uRatio);
  vec3 h=surface((pixels+uMargin)/uExtent);
  float altitude=h.x+h.y*uCosine+h.z*uSine;
  float elevation=(altitude-.037)*5.;
  float level=floor(elevation+.5);
  vec2 slope=vec2(dFdx(elevation),dFdy(elevation));
  float gradient=max(length(slope),.00001);
  float distance=abs(elevation-level)/gradient;
  bool major=mod(abs(level),4.)<.5;
  float width=1.1*uRatio;
  float ink=1.-smoothstep(max(0.,width*.5-.6),width*.5+.6,distance);
  float alpha=ink*.85;
  vec3 pigment=major?uAccentColor:uLineColor;
  color=vec4(pigment*alpha,alpha);
}`;
