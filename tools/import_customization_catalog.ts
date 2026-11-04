/** Offline import of UI catalog DATA; never used by the native app or game. */
import { mkdirSync, readFileSync, writeFileSync, renameSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { nodeLibrary } from '../vendor/ara/scripts/node-assets';
import { DEFAULT_PARAMS, PRESETS, randomParams, sanitizeParams } from '../vendor/ara/src/core/params';
import { GARMENTS } from '../vendor/ara/src/core/garments/catalog';
import { OUTFIT_SETS } from '../vendor/ara/src/core/garments/sets';
import { CARRIED, CARRY_LABELS, CARRY_STYLES } from '../vendor/ara/src/core/props/carry';
import { WEAPONS, WEAPON_LABELS } from '../vendor/ara/src/core/props/weapons';
import { HAIRSTYLES, FACIAL_HAIR, FACIAL_HAIR_LABELS, TATTOOS, TATTOO_LABELS, EARRINGS, EARRING_LABELS } from '../vendor/ara/src/core/looks';
import { EXPRESSIONS, FACE_SHAPES, EXPRESSION_WEIGHTS } from '../vendor/ara/src/core/face';
import { CLIPS, clipById } from '../vendor/ara/src/core/anim/clips';

const [command, input] = process.argv.slice(2);
const lib = await nodeLibrary();
const item = (id: string, name = id.replaceAll('_', ' ')) => ({ id, name });
const none = [item('none', 'None')];
const select = (id: string, label: string, category: string, choices: {id:string;name:string}[]) => ({ id, label, category, kind: 'choice', choices });
const numeric = (id: string, label: string, min = 0, max = 1) => ({ id, label, category: 'Body', kind: 'number', min, max });
const groups: Record<string, string[]> = {
  Skin: ['skinTint'], Hair: ['hairTint', 'hairColor', 'beardColor'], Looks: ['tattooInk'], Clothing: ['outfitTint'],
};
const fields: any[] = [
  {id:'name', label:'Character name', category:'Body', kind:'text'},
  {...numeric('seed','Seed',0,1e9),kind:'integer'},
  numeric('gender','Female to male'), numeric('age','Age (MakeHuman scale)',0.20982142857142858,1),
  numeric('muscle','Muscle'), numeric('weight','Weight'), numeric('height','Height'), numeric('proportions','Proportions'),
  numeric('african','African heritage'), numeric('asian','Asian heritage'), numeric('caucasian','Caucasian heritage'),
  numeric('breastSize','Breast size'), numeric('breastFirmness','Breast firmness'),
  select('skin','Skin','Skin',[item('auto','Automatic'), ...lib.catalog.skins]),
  select('eyes','Eye color','Face',lib.catalog.eyes),
  select('eyebrows','Eyebrows','Face',[...none,...lib.catalog.eyebrows]),
  select('eyelashes','Eyelashes','Face',[...none,...lib.catalog.eyelashes]),
  select('teeth','Teeth','Face',[...none,...lib.catalog.teeth]),
  select('expression','Expression','Face',EXPRESSIONS.map(id=>item(id))),
  select('hair','Hair style','Hair',[...none,...lib.catalog.hair,...HAIRSTYLES]),
  select('facialHair','Facial hair','Hair',FACIAL_HAIR.map(id=>item(id,FACIAL_HAIR_LABELS[id]))),
  select('earrings','Earrings','Looks',EARRINGS.map(id=>item(id,EARRING_LABELS[id]))),
  select('earringMetal','Earring metal','Looks',['gold','silver'].map(id=>item(id))),
  {...select('tattoos','Tattoos','Looks',TATTOOS.map(id=>item(id,TATTOO_LABELS[id]))),kind:'multi'},
  select('outfit','Outfit set','Clothing',[item('none','Separate pieces'),...lib.catalog.outfits,...OUTFIT_SETS.map(g=>item(g.id,g.label))]),
  select('top','Top','Clothing',[...none,...GARMENTS.filter(g=>g.slot==='top'&&!g.setOnly).map(g=>item(g.id,g.label))]),
  select('jacket','Jacket','Clothing',[...none,...GARMENTS.filter(g=>g.slot==='jacket'&&!g.setOnly).map(g=>item(g.id,g.label))]),
  select('bottom','Bottom','Clothing',[...none,...GARMENTS.filter(g=>g.slot==='bottom'&&!g.setOnly).map(g=>item(g.id,g.label))]),
  select('shoes','Footwear','Clothing',[...none,item('sandals','Sandals'),item('combat_boots','Combat boots'),item('flipflops','Flip-flops'),...lib.catalog.shoes,...GARMENTS.filter(g=>g.slot==='feet').map(g=>item(g.id,g.label))]),
  select('hat','Headwear','Clothing',[...none,...lib.catalog.hats,...GARMENTS.filter(g=>g.slot==='hat').map(g=>item(g.id,g.label))]),
  {...select('accessories','Accessories','Clothing',GARMENTS.filter(g=>g.slot==='acc'&&!g.setOnly).map(g=>item(g.id,g.label))),kind:'multi'},
  select('weapon','Held weapon','Props',WEAPONS.map(id=>item(id,WEAPON_LABELS[id]))),
  select('carry','Bag','Props',CARRIED.map(id=>item(id,CARRY_LABELS[id]))),
  select('vehicle','Vehicle','Props',[item('none','None'),item('bicycle','Bicycle (riding preview)')]),
  select('detail','Mesh detail','Rig',['low','medium','high'].map(id=>item(id))),
  select('rig','Skeleton','Rig',['game','game_lite','full','none'].map(id=>item(id))),
  select('restPose','Bind pose','Rig',['A','T'].map(id=>item(id))),
];
for (const [category, ids] of Object.entries(groups)) for (const id of ids) fields.push({id,label:id.replace(/([A-Z])/g,' $1'),category,kind:'color'});
for (const [id,label,category] of [
  ['underwear','Underwear','Clothing'],['tongue','Tongue','Face'],['faceBones','Jaw and eye bones','Rig'],
  ['animations','Include animations in exports','Rig'],['faceShapes','Facial blend shapes','Rig'],['speechShapes','Lip-sync visemes','Rig'],
]) fields.push({id,label,category,kind:'boolean'});

if (command === 'catalog') {
  // Fail loudly if upstream adds a parameter without a native control.
  const covered = new Set([...fields.map(f=>f.id),'modifiers','garmentStyles']);
  const missing = Object.keys(DEFAULT_PARAMS).filter(k=>!covered.has(k));
  if (missing.length) throw new Error(`Unmapped customization fields: ${missing.join(', ')}`);
  const schema = {
    face_shapes:FACE_SHAPES,expression_weights:EXPRESSION_WEIGHTS,reference_commit:'a4947ffdd14acf2ff018cb3568c941595f86d86e', defaults:DEFAULT_PARAMS, fields,
    presets:Object.entries(PRESETS).map(([name,p])=>({name,params:sanitizeParams({...DEFAULT_PARAMS,...p,name})})),
    sliders:lib.catalog.sliders,
    garments:[...GARMENTS.map(({id,label,patterns,color,color2,slot,colorLabels,setOnly})=>({id,label,patterns,color,color2,slot,colorLabels,setOnly})),
      ...OUTFIT_SETS.filter(g=>g.patterns).map(g=>({...g,slot:'set'})),
      ...Object.entries(CARRY_STYLES).map(([id,s])=>({id,label:CARRY_LABELS[id as keyof typeof CARRY_LABELS],slot:'bag',...s}))],
    outfit_sets:OUTFIT_SETS,
    clips:CLIPS.map(({id,name,category,loop,duration})=>({id,name,category,loop,duration})),
  };
  mkdirSync(dirname(input),{recursive:true}); writeFileSync(input,JSON.stringify(schema,null,2));
  console.log(JSON.stringify({fields:fields.length,sliders:lib.catalog.sliders.reduce((n,g)=>n+g.sliders.length,0),clips:CLIPS.length}));
}
