/**
 * Offline conversion of Ara's procedural wardrobe into body-bound templates.
 * This is intentionally a build tool: the Rust runtime only reads the result.
 *
 * Run from character_editor after `vendor/ara` dependencies are installed:
 *   vendor/ara/node_modules/.bin/tsx tools/bake_wardrobe.ts
 */
import { gzipSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { nodeLibrary } from '../vendor/ara/scripts/node-assets';
import { GARMENTS, buildGarments, dressing, outfitsUsed } from '../vendor/ara/src/core/garments';
import { OUTFIT_SETS } from '../vendor/ara/src/core/garments/sets';
import { makeBody } from '../vendor/ara/src/core/garments/body';
import { vertexNormals } from '../vendor/ara/src/core/geometry';
import { DEFAULT_PARAMS } from '../vendor/ara/src/core/params';
import { earrings, metalMaterial } from '../vendor/ara/src/core/looks/jewelry';
import { beardCards, buzzCards, cornrows, boxBraids, spikes } from '../vendor/ara/src/core/looks/hair';
import { buildWeapon, WEAPONS } from '../vendor/ara/src/core/props/weapons';
import { buildCarry, CARRIED } from '../vendor/ara/src/core/props/carry';
import { PATTERNS, makePattern } from '../vendor/ara/src/core/garments/textures';
import { Animator } from '../vendor/ara/src/core/anim/evaluate';
import { qconj, qmul, qrot } from '../vendor/ara/src/core/anim/quat';
import { placeBones, quatOf, skeletonDefs } from '../vendor/ara/src/core/rig/rig';
import { socketBone } from '../vendor/ara/src/core/rig/socket';

type Part = { name:string; positions:number[][]; indices:number[]; uvs:number[][]; refs:number[][]; weights:number[][]; rigid_joint?:string; material:any; hidden?:number[]; hidesHair?:boolean };
const out: { version:number; parts:Record<string,Part[]>; sets:Record<string,string[]>; hidden_faces:Record<string,number[]> } = {version:2,parts:{},sets:{},hidden_faces:{}};
const lib = await nodeLibrary();
const coords = lib.base.arrays.positions as Float32Array;
const fv = lib.base.arrays.faceVerts as Uint32Array;
const full = { positions:coords, faceSizes:new Uint8Array(fv.length / 4).fill(4), faceVerts:fv, faceUVs:lib.base.arrays.faceUVs as Uint32Array, uvs:lib.base.arrays.uvs as Float32Array };
const normals = vertexNormals(full);
const regions = lib.base.arrays.regions as Uint8Array;
const body = makeBody(coords,normals,regions,lib.base.meta.joints);

function nearest3(p:number[]): [number[],number[]] {
  const best:[number,number][] = [[Infinity,0],[Infinity,0],[Infinity,0]];
  for(let i=0;i<coords.length/3;i++) {
    if(regions[i] === 255) continue;
    const d=(coords[i*3]-p[0])**2+(coords[i*3+1]-p[1])**2+(coords[i*3+2]-p[2])**2;
    if(d<best[2][0]) { best[2]=[d,i]; best.sort((a,b)=>a[0]-b[0]); }
  }
  const inv=best.map(([d])=>1/Math.max(d,1e-8)); const sum=inv[0]+inv[1]+inv[2];
  return [best.map(x=>x[1]),inv.map(x=>x/sum)];
}
function convert(name:string, x:any, material:any):Part {
  const positions:number[][]=[]; const refs:number[][]=[]; const weights:number[][]=[];
  for(let i=0;i<x.positions.length;i+=3) { const p=[x.positions[i],x.positions[i+1],x.positions[i+2]]; positions.push(p); const ref=x.ref?.[i/3]; if(ref!==undefined&&ref>=0) { refs.push([ref,ref,ref]); weights.push([1,0,0]); } else { const [r,w]=nearest3(p); refs.push(r); weights.push(w); } }
  const indices:number[]=[]; for(let f=0,c=0;f<x.faceSizes.length;f++) { const n=x.faceSizes[f]; for(let k=1;k<n-1;k++) indices.push(x.faceVerts[c],x.faceVerts[c+k],x.faceVerts[c+k+1]); c+=n; }
  const uvs=(Array.from({length:x.positions.length/3},()=>[0,0]) as number[][]);
  for(let c=0;c<x.faceVerts.length;c++) { const v=x.faceVerts[c],u=x.faceUVs[c]; if(u>=0) uvs[v]=[x.uvs[u*2]??0,x.uvs[u*2+1]??0]; }
  return {name,positions,indices,uvs,refs,weights,material:{...material, map:material.map ? `textures/${material.map}`:undefined, normalMap:material.normalMap ? `textures/${material.normalMap}`:undefined}};
}
function add(key:string, pieces:any[], rigidJoint?:string) {
  out.parts[key]=pieces.filter(Boolean).map((p,i)=>({ ...convert(`${key}_${i}`,p.mesh,p.material), ...(rigidJoint ? {rigid_joint:rigidJoint} : {}) }));
  // Keep quad IDs, including for procedural footwear. Expanding these masks
  // into vertex sets would erase neighboring skin around each garment edge.
  const hidden=new Set<number>();
  for(const p of pieces) if(p?.hides) p.hides.forEach((covered:number,face:number)=>{if(covered)hidden.add(face);});
  out.hidden_faces[key]=[...hidden];
}
type V = [number,number,number];
const sub=(a:V,b:V):V=>[a[0]-b[0],a[1]-b[1],a[2]-b[2]];
const dot=(a:V,b:V)=>a[0]*b[0]+a[1]*b[1]+a[2]*b[2];
const cross=(a:V,b:V):V=>[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]];
const norm=(v:V):V=>{const n=Math.hypot(...v)||1;return [v[0]/n,v[1]/n,v[2]/n];};
/** Exact Ara WeaponR frame, expressed in the MakeHuman decimetre body space. */
function weaponFrame(wrist:V,index:V,middle:V,pinky:V,thumbTip:V) {
  let palm=norm(cross(sub(index,wrist),sub(pinky,wrist)));
  if(dot(sub(thumbTip,wrist),palm)<0) palm=[-palm[0],-palm[1],-palm[2]];
  const y=norm(sub(index,pinky)), forward=norm(sub(middle,wrist));
  const z=norm([forward[0]-y[0]*dot(forward,y),forward[1]-y[1]*dot(forward,y),forward[2]-y[2]*dot(forward,y)]);
  const x=cross(y,z), mid:V=[(index[0]+pinky[0])/2,(index[1]+pinky[1])/2,(index[2]+pinky[2])/2];
  return {head:[mid[0]+palm[0]*.24-forward[0]*.12,mid[1]+palm[1]*.24-forward[1]*.12,mid[2]+palm[2]*.24-forward[2]*.12] as V,x,y,z};
}
/** Ara props are authored in metres in their socket-local frame. `x`, `y`,
 * and `z` are frame columns, so each output component takes one component
 * from every column. */
function socketBound(pieces:any[], frame:{head:V,x:V,y:V,z:V}) {
  return pieces.map(p=>({ ...p, mesh:{...p.mesh,positions:Float32Array.from(p.mesh.positions.map((v:number,i:number)=>{
    const axis=i%3, base=i-axis;
    return frame.head[axis]+(frame.x[axis]*p.mesh.positions[base]+frame.y[axis]*p.mesh.positions[base+1]+frame.z[axis]*p.mesh.positions[base+2])*10;
  }))} }));
}
/** Apply Ara's weapon-frame fit before mapping the local weapon into WeaponR. */
function rotatePieces(pieces:any[], q:[number,number,number,number]) {
  return pieces.map(p=>({ ...p, mesh:{...p.mesh,positions:Float32Array.from(p.mesh.positions.map((_:number,i:number)=>{
    const base=i-i%3, r=qrot(q,[p.mesh.positions[base],p.mesh.positions[base+1],p.mesh.positions[base+2]]);
    return r[i%3];
  }))} }));
}
// The source character places carried props in the relaxed, closed left grip,
// then binds them rigidly to LeftHand.  Preserve that authored bind-space
// placement rather than pretending a bag shares the right-hand weapon socket.
const minY=Math.min(...Array.from(coords).filter((_,i)=>i%3===1));
const sourceJoint=(name:string):V=>{
  const ids=lib.base.meta.joints[name] as number[];
  const sum:V=[0,0,0];
  for(const index of ids) { sum[0]+=coords[index*3]*.1; sum[1]+=(coords[index*3+1]-minY)*.1; sum[2]+=coords[index*3+2]*.1; }
  return [sum[0]/ids.length,sum[1]/ids.length,sum[2]/ids.length];
};
const sourceBones=placeBones(skeletonDefs(lib,{type:'game',face:false}),sourceJoint);
const sourceRight=sourceBones.findIndex(b=>b.name==='RightHand');
const sourceHand={wrist:sourceJoint('wrist.R____head'),index:sourceJoint('finger2-1.R____head'),middle:sourceJoint('finger3-1.R____head'),pinky:sourceJoint('finger5-1.R____head'),thumbTip:sourceJoint('finger1-3.R____tail')};
sourceBones.push(socketBone(sourceHand,sourceRight,((Math.max(...Array.from(coords).filter((_,i)=>i%3===1))-minY)*.1)/1.8));
const sourceRig={type:'game' as const,restPose:'A' as const,bones:sourceBones,holds:['Left' as const]};
const sourceAnimator=new Animator(sourceRig);
function carryBound(pieces:any[]) {
  const hand=sourceAnimator.relaxedBone('LeftHand'), mid=sourceAnimator.relaxedBone('LeftHandMiddle1');
  const hi=sourceBones.findIndex(b=>b.name==='LeftHand');
  if(!hand||!mid||hi<0) throw new Error('source left-hand grip unavailable');
  const grip:V=[hand.pos[0]+(mid.pos[0]-hand.pos[0])*1.15-.018,hand.pos[1]+(mid.pos[1]-hand.pos[1])*1.15,hand.pos[2]+(mid.pos[2]-hand.pos[2])*1.15];
  const toBind=qmul(quatOf(sourceBones[hi].rot),qconj(hand.rot));
  return pieces.map(p=>({ ...p, mesh:{...p.mesh,positions:Float32Array.from(p.mesh.positions.map((_:number,i:number)=>{
    const base=i-i%3, w=qrot(toBind,[p.mesh.positions[base]+grip[0]-hand.pos[0],p.mesh.positions[base+1]+grip[1]-hand.pos[1],p.mesh.positions[base+2]+grip[2]-hand.pos[2]]);
    // Convert source metres/grounded coordinates back to native decimetres.
    return (sourceBones[hi].head[i%3]+w[i%3])/.1+(i%3===1?minY:0);
  }))} }));
}
function fitProxy(proxy:any):Float32Array { const {refs,weights,offsets}=proxy.arrays; const scale=proxy.meta.scale.map((s:any,axis:number)=>s?Math.abs(coords[s[0]*3+axis]-coords[s[1]*3+axis])/s[2]:1); const out=new Float32Array(proxy.meta.vertexCount*3); for(let i=0;i<proxy.meta.vertexCount;i++) for(let d=0;d<3;d++) out[i*3+d]=weights[i*3]*coords[refs[i*3]*3+d]+weights[i*3+1]*coords[refs[i*3+1]*3+d]+weights[i*3+2]*coords[refs[i*3+2]*3+d]+offsets[i*3+d]*scale[d]; return out; }
async function fittedOutfits(d:any) { const fitted:any={}; for(const id of outfitsUsed(d)) { const item=lib.catalog.outfits.find((x:any)=>x.id===id); if(!item?.file) continue; const proxy:any=await lib.proxy(item.file); const a:any=proxy.arrays, positions=fitProxy(proxy); const ref=Uint32Array.from({length:positions.length/3},(_,i)=>{ const [r]=nearest3([positions[i*3],positions[i*3+1],positions[i*3+2]]); return r[0]; }); fitted[id]={mesh:{positions,faceSizes:a.faceSizes,faceVerts:a.faceVerts,faceUVs:a.faceUVs,uvs:a.uvs},ref,deleteVerts:a.deleteVerts,normalMap:item.material?.normal}; } return fitted; }
for (const g of GARMENTS) {
  const p:any={...DEFAULT_PARAMS,underwear:false,outfit:'none',top:'none',bottom:'none',jacket:'none',shoes:'none',hat:'none',accessories:[],carry:'none',garmentStyles:{}};
  if(g.slot==='top')p.top=g.id; else if(g.slot==='bottom')p.bottom=g.id; else if(g.slot==='jacket')p.jacket=g.id; else if(g.slot==='feet')p.shoes=g.id; else if(g.slot==='hat')p.hat=g.id; else if(g.slot==='bag')p.carry=g.id; else p.accessories=[g.id];
  const d=dressing(p), pieces=buildGarments(lib,coords,normals,fv,d,await fittedOutfits(d)); add(g.id,pieces, g.id==='backpack' ? 'Spine2' : undefined);

}
// These three footwear choices are procedural built-ins rather than catalog
// entries, but are valid `shoes` parameter IDs and therefore need templates.
for (const id of ['sandals','boots','flipflops']) {
  const p:any={...DEFAULT_PARAMS,underwear:false,outfit:'none',top:'none',bottom:'none',jacket:'none',shoes:id==='boots'?'combat_boots':id,hat:'none',accessories:[],carry:'none',garmentStyles:{}};
  add(id,buildGarments(lib,coords,normals,fv,dressing(p)));
}
for(const s of OUTFIT_SETS) { out.sets[s.id]=[s.top,s.bottom,s.jacket,...s.accessories,s.shoes,s.hat].filter((x):x is string=>!!x&&x!=='none'); }
for(const id of ['studs','small_hoops','big_hoops']) add(`earrings:${id}`,earrings(id as any,body,coords,normals).map(mesh=>({mesh,material:metalMaterial('gold')})));
const hctx:any={base:body,coords,normals,regions,faceVerts:fv,seed:1,low:false};
for(const [id,fn] of Object.entries({buzz:()=>buzzCards(hctx),mohawk:()=>spikes(hctx),cornrows:()=>cornrows(hctx),box_braids:()=>boxBraids(hctx,body.L.neckY+.25)})) { const mesh=fn(); if(mesh)add(`hair:${id}`,[{mesh,material:{name:'Hair',color:'#2a1e16',roughness:.6,metalness:0,alpha:'mask',doubleSided:true}}]); }
for(const id of ['mustache','mustache_stubble','goatee','short_beard','full_beard']) { const mesh=beardCards(hctx,id as any); if(mesh)add(`facialHair:${id}`,[{mesh,material:{name:'Facial hair',color:'#241a13',roughness:.7,metalness:0,alpha:'mask',doubleSided:true}}]); }
const rightFrame=weaponFrame(body.joint('wrist.R____head'),body.joint('finger2-1.R____head'),body.joint('finger3-1.R____head'),body.joint('finger5-1.R____head'),body.joint('finger1-3.R____tail'));
for(const id of WEAPONS) if(id!=='none') {
  const fit=id==='rifle'||id==='pistol'?sourceAnimator.fitFor(id):undefined;
  const pieces=buildWeapon(id as any,fit?{supportZ:fit.supportZ}:undefined).map(x=>({mesh:x.mesh,material:x.material}));
  add(`weapon:${id}`,socketBound(fit?rotatePieces(pieces,fit.rot):pieces,rightFrame),'WeaponR');
}
// Backpack is a worn garment above; buildCarry intentionally returns no hand prop for it.
for(const id of CARRIED) if(id!=='none'&&id!=='backpack') {
  const pieces=buildCarry(id as any,()=>({map:'',alpha:false}),{}).map(x=>({mesh:x.mesh,material:x.material}));
  add(`carry:${id}`,carryBound(pieces),'LeftHand');
}
// Reusable neutral fabric masks. Rust applies selected colours at runtime; these
// masks give every pattern choice a distinct real texture without Node at play.
const textureDir=resolve(import.meta.dirname,'../assets/native/textures'); mkdirSync(textureDir,{recursive:true});
for(const pattern of PATTERNS) { const t=makePattern(pattern,'#ffffff','#606060'); writeFileSync(resolve(textureDir,`wardrobe_pattern_${pattern}.png`),t.color); if(t.normal)writeFileSync(resolve(textureDir,`wardrobe_pattern_${pattern}_n.png`),t.normal); }
const destination=resolve(import.meta.dirname,'../assets/native/wardrobe.json.gz'); mkdirSync(resolve(destination,'..'),{recursive:true}); writeFileSync(destination,gzipSync(JSON.stringify(out))); console.log(`wrote ${destination}: ${Object.keys(out.parts).length} templates`);
