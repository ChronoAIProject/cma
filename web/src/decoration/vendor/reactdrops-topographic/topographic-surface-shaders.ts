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
precision highp sampler2DArray;
uniform sampler2D uHeight, uGrain;
uniform sampler2DArray uCuts;
uniform vec2 uGrid, uExtent, uCanvasSize;
uniform float uGrainAmount;
uniform float uGradientDiffusion;
uniform float uMargin, uRatio, uCosine, uSine, uDensity, uWidth, uOpacity;
uniform float uDashLength, uDashGap;
uniform vec4 uStamps[1];
uniform highp int uStampCount;
uniform float uPointerRadius, uPointerLift, uHoverTime;
uniform highp int uHoverEffect;
uniform vec2 uPointerDrag;
uniform int uStyle, uLevelOffset, uLayers[41], uGradientEnabled, uGradientStops;
uniform vec3 uLineColor, uAccentColor, uGradientColors[8];
out vec4 color;
vec4 weights(float t) {
  float t2=t*t, t3=t2*t;
  return vec4(1.-3.*t+3.*t2-t3, 4.-6.*t2+3.*t3, 1.+3.*t+3.*t2-3.*t3, t3)/6.;
}
// Four hardware bilinear taps evaluate a tensor cubic B-spline exactly.
// The height and its first two spatial derivatives stay continuous at cells.
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
// Reconstruct the material phase smoothly too; bilinear derivatives would
// make cut strength flicker at atlas texel boundaries during deformation.
vec2 cutPhase(vec2 uv, float layer) {
  vec2 grid=vec2(textureSize(uCuts,0).xy), p=uv*grid-.5;
  vec2 cell=floor(p), f=fract(p);
  vec4 x=weights(f.x), y=weights(f.y);
  vec2 gx=vec2(x.x+x.y,x.z+x.w), gy=vec2(y.x+y.y,y.z+y.w);
  vec2 px=cell.x+vec2(-1.+x.y/gx.x,1.+x.w/gx.y);
  vec2 py=cell.y+vec2(-1.+y.y/gy.x,1.+y.w/gy.y);
  vec2 a=texture(uCuts,vec3((vec2(px.x,py.x)+.5)/grid,layer)).rg;
  vec2 b=texture(uCuts,vec3((vec2(px.y,py.x)+.5)/grid,layer)).rg;
  vec2 c=texture(uCuts,vec3((vec2(px.x,py.y)+.5)/grid,layer)).rg;
  vec2 d=texture(uCuts,vec3((vec2(px.y,py.y)+.5)/grid,layer)).rg;
  return mix(mix(a,b,gx.y),mix(c,d,gx.y),gy.y)*2.-1.;
}
vec3 elevationColor(float height) {
  float intervals=float(uGradientStops-1);
  if (uGradientDiffusion<=0.) {
    float position=height*intervals;
    int segment=min(int(floor(position)),uGradientStops-2);
    return mix(uGradientColors[segment],uGradientColors[segment+1],
      smoothstep(0.,1.,position-float(segment)));
  }
  // Widen transitions in elevation space. Their overlapping weights blend
  // neighboring colors while retaining the exact terrain and contour height.
  float halfWidth=(1.+3.*uGradientDiffusion)/intervals*.5;
  vec3 terrain=uGradientColors[0];
  for (int i=0; i<7; i++) {
    if (i>=uGradientStops-1) break;
    float center=(float(i)+.5)/intervals;
    terrain+=(uGradientColors[i+1]-uGradientColors[i])
      *smoothstep(center-halfWidth,center+halfWidth,height);
  }
  return terrain;
}
void main() {
  // Canvas coordinates are top-down; the cached terrain covers overscan too.
  vec2 pixels=vec2(gl_FragCoord.x/uRatio, (uExtent.y-2.*uMargin)-gl_FragCoord.y/uRatio);
  // The pointer reshapes the contours; heights and colors follow. Every
  // effect fades with distance and none can fold the terrain over itself.
  float raise=0.;
  if(uStampCount>0) {
    vec2 toPointer=pixels-uStamps[0].xy;
    float spread=uPointerRadius*uPointerRadius, distance2=dot(toPointer,toPointer);
    if(distance2<9.*spread) {
      float weight=uStamps[0].z*exp(-distance2/spread);
      if(uHoverEffect==1) {
        // Swirl: rotate about the pointer. Distances are kept, so no folds.
        float angle=uPointerLift*2.*weight, c=cos(angle), s=sin(angle);
        pixels=uStamps[0].xy+mat2(c,s,-s,c)*toPointer;
      } else if(uHoverEffect==2) {
        // Ripple: waves travel outward; small amplitude per wavelength.
        float distance=sqrt(distance2);
        float wave=sin(6.2831853*distance/(uPointerRadius*.6)-uHoverTime*4.);
        pixels-=uPointerLift*.09*uPointerRadius*wave*weight*toPointer/max(distance,.001);
      } else if(uHoverEffect==3) {
        // Rings: a hill under the pointer adds concentric contours.
        raise=uPointerLift*weight;
      } else if(uHoverEffect==4) {
        // Drag: contours follow the pointer's motion, then settle back.
        pixels-=weight*uPointerDrag;
      } else {
        // Spread: contours move away from the pointer.
        pixels-=uPointerLift*weight*toPointer;
      }
    }
  }
  vec2 uv=(pixels+uMargin)/uExtent;
  vec3 h=surface(uv);
  float altitude=h.x+h.y*uCosine+h.z*uSine+raise;
  float elevation=(altitude-.037)*uDensity;
  float level=floor(elevation+.5);
  vec2 slope=vec2(dFdx(elevation),dFdy(elevation));
  float gradient=max(length(slope),.00001);
  float distance=abs(elevation-level)/gradient;
  bool major=mod(abs(level),4.)<.5;
  float width=uWidth*uRatio;
  float ink=1.-smoothstep(max(0.,width*.5-.6),width*.5+.6,distance);
  bool broken=uStyle==2 || (uStyle==1 && major);
  if (broken) {
    int index=clamp(int(level)+uLevelOffset,0,40);
    int layer=uLayers[index];
    float period=uDashLength+uDashGap;
    float arc;
    vec2 arcGradient;
    float phaseQuality=1.;
    if (layer>=0) {
      vec2 phase=cutPhase(uv,float(layer));
      arc=atan(phase.y,phase.x)*period/6.28318530718;
      // Differentiate the complex phase, avoiding atan's wrap discontinuity.
      vec2 dx=dFdx(phase), dy=dFdy(phase);
      arcGradient=vec2(phase.x*dx.y-phase.y*dx.x,phase.x*dy.y-phase.y*dy.x)
        *period/(6.28318530718*max(dot(phase,phase),.001));
      phaseQuality=smoothstep(.15,.5,length(phase));
    } else {
      // New summit levels inherit a fixed material cut pattern as they emerge.
      arc=pixels.x*.70710678+pixels.y*.70710678;
      arcGradient=vec2(.70710678,-.70710678)/uRatio;
    }
    vec2 tangent=vec2(-slope.y,slope.x)/gradient;
    float materialSpeed=abs(dot(arcGradient,tangent))*uRatio;
    // A collapsed rest-coordinate projection must never stretch a small gap
    // into an empty summit arc. Fade unreliable cuts into a continuous stroke.
    // Both gates are spatial and continuous: no clock or moving dash offset.
    float cutStrength=phaseQuality*smoothstep(.45,.75,materialSpeed)
      *(1.-smoothstep(1.6,2.4,materialSpeed));
    float dashDistance=abs(mod(arc+period*.5,period)-period*.5);
    float aa=.55*max(materialSpeed,.7)/uRatio;
    float dash=1.-smoothstep(uDashLength*.5-aa,uDashLength*.5+aa,dashDistance);
    ink*=mix(1.,dash,cutStrength);
  }
  float alpha=ink*uOpacity;
  vec3 pigment=major?uAccentColor:uLineColor;
  float fleck=uGrainAmount>0.?texture(uGrain,gl_FragCoord.xy/uCanvasSize).r:0.;
  float grain=uGrainAmount*fleck;
  float white=uGrainAmount*.7*fleck*fleck*(1.-alpha);
  // The same stationary mote exposure textures the color and the contour ink.
  // Coverage still comes from the smooth elevation field, without a CSS mask.
  if (uGradientEnabled==1) {
    // The same altitude creates the color and the contour. Fixed elevation
    // bounds preserve the palette when density changes, without auto exposure.
    float height=clamp((altitude+.65)/1.3,0.,1.);
    vec3 terrain=elevationColor(height);
    terrain=mix(terrain,vec3(1.),uGrainAmount*.7*fleck*fleck);
    color=vec4(mix(terrain,pigment*(1.-grain*.4),alpha),1.);
  } else {
    color=vec4(pigment*(1.-grain*.4)*alpha+vec3(white),alpha+white);
  }
}`;
