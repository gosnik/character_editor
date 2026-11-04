/** Offline conversion of reference paint designs into portable monochrome masks. */
import {writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {nodeLibrary} from '../vendor/ara/scripts/node-assets';
import {baseBody,paintOverlay} from '../vendor/ara/src/core/looks/overlay';
import {tattooLayers,TATTOOS} from '../vendor/ara/src/core/looks/tattoos';
import {hairLayers} from '../vendor/ara/src/core/looks/hair';
import {hairCardTexture,braidTexture,spikeTexture} from '../vendor/ara/src/core/looks/textures';
const lib=await nodeLibrary();const root=resolve(import.meta.dirname,'../assets/native');const base=baseBody(lib);const masks:Record<string,number[]>={};
for(const id of [...TATTOOS,'stubble','buzz_scalp']){
 const layers=id==='stubble'?hairLayers('none','stubble','#ffffff','#ffffff'):id==='buzz_scalp'?hairLayers('buzz','none','#ffffff','#ffffff'):tattooLayers(base,[id as any],'#ffffff');
 const result=await paintOverlay(lib,layers,1024);writeFileSync(`${root}/textures/look_${id}.png`,result.png);masks[id]=Array.from(result.faces).flatMap((v,i)=>v?[i]:[]);console.log(id,masks[id].length);
}
for(const [name,make]of Object.entries({hair_card:hairCardTexture,hair_braid:braidTexture,hair_spike:spikeTexture})){const t=make();writeFileSync(`${root}/textures/look_${name}.png`,t.color);if(t.normal)writeFileSync(`${root}/textures/look_${name}_n.png`,t.normal);}
writeFileSync(`${root}/look_masks.json`,JSON.stringify(masks));
