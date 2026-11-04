# Crank & Citizen cast coverage

What Human Studio needs before it can rebuild every character in
`CrankCitizen/assets/characters/` (84 folders; 79 are referenced by
`crates/crank_citizen/src/customize.rs`). The audit was made on 2026-11-03 from
each character's `preview.png`, `info.json` and `citizens2.json`, against the editor
catalog (`human-cli catalog fields|garments|clips`) with **Cartoon hm08** mode. Outfit
calls are visual estimates from 512 px previews.

## Summary

| Verdict | Characters | Meaning |
| --- | ---: | --- |
| **Approximate now** | 8 | Built-in garments get close (substitutions noted); a recolour and a hairstyle choice away |
| **Needs garments / hair** | 60 | Body is fine (adult, teen or senior); specific garments, hairstyles or glasses are missing |
| **Needs under-12 bodies** | 12 | Preschool to 11-year-olds; the editor's age floor is 12 (plus garments) |
| **Fantasy gear** | 2 | Alchemists: layered leather, bandoliers and many props; possible later via the wardrobe library |
| **Creature** | 2 | Werewolves: not a human body; keep the Meshy models |

Nothing in the cast is blocked by the Cartoon base itself. The rigged hm08 body
reaches the cast's proportions at adult, teen and senior ages. The gaps are age
range, wardrobe and pipeline.

## Missing editor features, ranked by characters unblocked

| # | Feature | Unblocks | Notes |
| ---: | --- | ---: | --- |
| 1 | **Hairstyle library** (stylised) | ~60 | Only 15 realistic styles. Needed: kids' pigtails, afro puffs and space buns, braided pigtails, curly and wavy long, 1920s finger-wave bob, Victorian updo and grey bun, 1950s ponytail with scarf, pompadour and quiff, slick side part, 1980s feathered and mullet, 1990s curtains, men's curly short, long men's hair. |
| 2 | **Knitwear**: crew sweater, cardigan, hoodie, sweater vest, sweatshirt | ~23 | The most common top in the cast (grandparents, mums, teens, 1940s/90s men). |
| 3 | **Long-sleeve button shirts** (plain, plaid flannel, 1970s print) and polos | ~20 | Built-ins are short-sleeve `camp_shirt`, `uniform_shirt_long` and `silk_blouse`. |
| 4 | **Dresses and skirts**: pleated, swing, A-line midi, long Victorian, 1920s fringe, 1950s shirt-dress, 1960s shift, pinafore, floral | ~16 | Built-ins are only `pencil_skirt_*`, wrappers and gowns. |
| 5 | **Jackets**: denim, bomber/varsity, track, corduroy, windbreaker, blazer, lab coat, chore/work coat, puffer vest | ~16 | `suit_jacket` and `overcoat` only; an Engineer coat draft exists as a custom garment. |
| 6 | **Under-12 age range** | 12 | The age field minimum is 0.2098 (12 years), inherited from Ara. MakeHuman ships baby and child targets. Needs the floor lowered, child clothing/underwear rules revisited, and the cartoon child weights tuned (the head/body/chest split already exists). |
| 7 | **Waistcoats, braces and suspenders, three-piece suits** | ~9 | 1890s–1940s men; an Alchemist waistcoat draft exists. |
| 8 | **Glasses** (round, rectangular, reading) | 7 | Only sunglasses, aviators and goggles exist. |
| 9 | **Overalls/dungarees** (one-piece) | 6 | Kids, farmer, overalls woman. |
| 10 | **Hats and hair accessories**: flat/newsboy cap, cloche, straw sun hat, bows, headbands, hair ribbons | ~10 | `baseball_cap`, fedoras, `beret` and `cowboy_hat` exist. |
| 11 | **Work/role sets**: chef jacket with neckerchief and apron, police cap and badge, lab coat | 6 | `uniform_shirt`, `uniform_trousers`, `utility_belt` and `gloves` already cover most of the guards. |
| 12 | **Footwear**: Mary Janes, canvas high-tops | ~10 | Sneakers, boots, loafers, oxfords and heels are already fine. |

**Biggest lever: the wardrobe library.** `game_assets/generated/wardrobe-library`
already inventories **684 garments from 88 turnaround sheets**, the same sheets the cast
was made from: 225 are full-resolution verified, 86 have draft meshes, and **4** are
fitted editor garments (`assets/native/custom_garments/`). Turning that inventory into
fitted custom garments covers rows 2–5, 7, 9 and 10 for the cast directly, with the
right look. That is better than authoring generic procedural garments one by one.

## Game integration findings

`CrankCitizen/tools/generic_characters.py` exports editor characters to
`assets/characters_generic/` (used by `CC_DEMO=wardrobe`, not yet in the web
manifest). Status after the pipeline pass (2026-11-03):

1. **Cartoon mode** — done. Exports use the single Cartoon mode (the separate
   Meshy-surface mode was removed from the editor).
2. **Clips** — done. The ten clips in `skinned::CLIP_*` order, with
   `ual1:Dance_Loop` in the dance slot instead of `laugh`.
3. **Rig** — works. `game_lite` has 28 Mixamo-named bones; the game finds `Hips` and
   `RightHand` by name. Cartoon limbs are re-posed to hm08's rest angles, so arms hang
   and swing like the Meshy cast in idle and walk. Sit/sleep hip offsets still need
   checking per body height.
4. **Recolouring** — done. `pack` writes `regions.png` (skin, hair, top, bottom, shoes)
   from mesh names and garment slots, plus `region_luma` in `info.json`. All textures
   share one atlas so the mask matches every mesh. The game doesn't read these for
   generic characters yet (they're not in `assets/manifest.json`).
5. **Budget** — done. Blender collapse decimation (body cut deeper than garments,
   garment layers pushed apart afterwards) brings every character to about 21k
   triangles and 1.9–2.8 MB, against the Meshy median of 20.7k and 2.7 MB.
6. **Heights** — `GenericRoot` scale. Kids still need under-12 bodies.
7. **Faces** — done. Cartoon mode paints flat skin, cartoon eyes (white sclera, large
   dark iris, highlight), solid brows and lashes and matte beards. Eye openings are wider,
   and arms are spread to clear the wider cartoon torso in every clip. Hair uses
   solid cartoon shells with carved locks (`tools/cartoon_hair`), and lashes a solid lid line.
8. **Unreferenced cast folders:** `guard_female`, `man_1970s`, `woman_1960s`,
   `woman_1970s` and `woman_1980s` are not in `customize.rs` model tables (lowest
   priority).

The eight "approximate now" characters have recipes in `generic_characters.py` under
their cast names and heights, for side-by-side comparison in `CC_DEMO=wardrobe`.

## Per-character table

Legend: **✓** available now (built-in or close substitute) · **✗** missing ·
*sub* = substitution. Ages are visual estimates.

| Character | Age / sex | Verdict | Wardrobe and features |
| --- | --- | --- | --- |
| alchemist_female | adult F | Fantasy gear | ✗ corset, layered coat, bandolier with vials (✓ `amulet_bandolier` *sub*), ✗ hairband, ✓ boots, ✓ gloves |
| alchemist_male | adult M | Fantasy gear | ✗ long ornate coat, ✗ glasses, ✓ `utility_belt`, ✓ gloves, ✓ boots, props |
| arthur | senior M | Needs garments | ✗ long-sleeve shirt over tee, ✓ `trousers`, ✓ shoes, ✓ `full_beard`, ✗ bald-with-fringe hair |
| auntie | adult F | Needs garments | ✗ cardigan, ✓ `silk_blouse` *sub*, ✓ `trousers`, ✗ wavy grey shoulder hair |
| auntie_1970s | adult F | Needs garments | ✗ 1970s print blouse, ✗ midi skirt, ✓ `belt`, ✗ short curly hair |
| bearded_man | adult M | Needs garments | ✗ cardigan, ✗ henley, ✓ `trousers`, ✓ `combat_boots` *sub*, ✓ `full_beard` |
| ben | adult M | **Approx. now** | ✗ polo (✓ `camp_shirt` *sub*), ✓ `baggy_jeans`, ✓ `sneakers`, ✓ `short_beard`, ✓ `short02` |
| cardigan_woman | adult F | Needs garments | ✗ wrap cardigan, ✓ `trousers`, ✓ `sneakers`, ✗ long straight hair (✓ `long01` *sub*) |
| cook_female | adult F | Needs garments | ✗ chef blouse, ✗ apron dress, ✗ neckerchief, ✗ hair bow, props |
| cook_male | adult M | Needs garments | ✗ shirt with neckerchief, ✗ leather apron with tools, ✓ `combat_boots`, ✓ `short_beard` |
| curly_woman | adult F | Needs garments | ✗ long-sleeve blouse, ✗ wide mustard trousers (✓ `trousers` *sub*), ✓ `loafers`, ✗ long curly hair |
| dad_1970s | adult M | Needs garments | ✗ corduroy jacket, ✗ open-collar shirt, ✓ `trousers`, ✓ `mustache`, ✗ feathered 70s hair |
| denim_man | adult M | Needs garments | ✗ denim jacket, ✓ `oversized_tee` *sub*, ✓ `baggy_jeans`, ✓ boots, ✓ `stubble` |
| engineer_female | adult F | Needs garments | ✓ Engineer coat/trousers/sneakers **custom drafts**, ✓ `goggles`, ✓ `utility_belt`, ✗ ponytail with fringe (✓ `ponytail01` *sub*) |
| engineer_male | adult M | Needs garments | ✓ Engineer coat draft, ✗ sweater under coat, ✗ glasses, ✓ `trousers`, ✓ boots |
| farmer_female | adult F | Needs garments | ✗ plaid shirt, ✗ dungarees, ✗ straw sun hat, ✗ neckerchief, ✓ boots, ✓ `gloves` |
| farmer_male | adult M | Needs garments | ✗ plaid shirt, ✗ work vest, ✓ `baggy_jeans`, ✓ `utility_belt`, ✓ `gloves`, ✓ boots |
| flapper_1920s | adult F | Needs garments | ✗ fringe drop-waist dress, ✗ cloche hat, ✓ `pearl_necklace`, ✗ Mary Janes, ✗ finger-wave bob |
| gent_1920s | adult M | Needs garments | ✗ tan waistcoat/cardigan, ✓ shirt + tie (`set_suit_tie` pieces), ✓ `suit_trousers`, ✓ `oxfords`, ✓ `mustache` |
| girl_1900s | child F | Under-12 | ✗ pinafore over dress, ✗ long braid/ribbon hair, ✓ boots |
| grandad | senior M | Needs garments | ✗ crew sweater over shirt, ✗ glasses, ✓ `trousers`, ✓ `full_beard`, ✗ bald-with-fringe |
| grandpa | senior M | Needs garments | ✗ crew sweater, ✗ glasses, ✓ `trousers`, ✓ `full_beard`, ✓ `loafers` |
| granny_1920s | senior F | Needs garments | ✗ cardigan, ✗ long pleated skirt, ✗ Mary Janes, ✗ grey curly updo |
| guard_female | adult F | **Approx. now** | ✓ `uniform_shirt`, ✓ `uniform_trousers`, ✓ `utility_belt`, ✓ `gloves`, ✓ `baseball_cap` (✗ badge text), ✓ `combat_boots` |
| guard_male | adult M | **Approx. now** | as guard_female; ✓ `short02` |
| human_female | child F | Under-12 | ✗ adventure blouse, ✗ goggles on head (✓ `goggles`), ✓ `utility_belt`, ✗ cuffed trousers, ✗ bandana hair |
| human_male | child M | Under-12 | ✗ scarf, ✗ layered tunic, ✓ `arm_wraps`, ✓ boots, props |
| kid_boy | child M | Under-12 | ✓ `oversized_tee`, ✓ `cargo_shorts`, ✓ `sneakers`, ✗ messy hair |
| kid_girl | child F | Under-12 | ✗ dungarees, ✗ long-sleeve top, ✓ `sneakers`, ✓ `long01` *sub* |
| lady_1890s | adult F | Needs garments | ✗ puff-sleeve blouse, ✗ long skirt, ✓ `belt`, ✗ updo |
| lin | adult F | Needs garments | ✗ cardigan, ✓ tee, ✓ `trousers`, ✓ `bob02` |
| little_boy | child M | Under-12 | ✓ tee, ✓ `cargo_shorts`, ✓ `sneakers`, ✗ messy hair |
| little_girl | child F | Under-12 | ✗ dungarees, ✓ tee, ✓ `sneakers`, ✗ afro puffs |
| malik | adult M | Needs garments | ✗ zip hoodie, ✓ tee, ✓ chinos (`trousers`), ✓ `sneakers`, ✓ `short01` |
| man_1910s | adult M | Needs garments | ✗ waistcoat over long-sleeve shirt, ✓ `trousers`, ✓ boots, ✓ `mustache` |
| man_1930s | adult M | Needs garments | ✗ braces, ✗ long-sleeve shirt, ✓ `trousers`, ✓ boots, ✗ slick side part |
| man_1940s | adult M | Needs garments | ✗ sweater vest, ✗ plaid shirt + tie, ✓ `trousers`, ✓ `oxfords` |
| man_1950s | adult M | **Approx. now** | ✓ `camp_shirt`, ✓ `trousers`, ✓ `belt`, ✓ `loafers`, ✗ quiff (✓ `short04` *sub*) |
| man_1960s | adult M | **Approx. now** | ✗ fitted knit polo (✓ `oversized_tee` *sub*), ✓ `trousers`, ✓ `belt`, ✓ shoes, ✗ mop-top |
| man_1970s | adult M | Needs garments | ✗ 1970s print shirt, ✗ flares, ✓ `belt`, ✓ `afro01`, ✓ `mustache` |
| man_1980s | adult M | Needs garments | ✗ track jacket, ✗ track pants, ✓ `sneakers`, ✓ `short_beard` |
| man_1990s | adult M | Needs garments | ✗ crew sweater, ✗ cargo pants, ✓ `sneakers`, ✗ long curly hair, ✓ `goatee` |
| man_2000s | adult M | **Approx. now** | ✓ `camp_shirt` over tee, ✗ cargo pants (✓ `baggy_jeans` *sub*), ✓ `sneakers`, ✗ spiky hair |
| maya | adult F | **Approx. now** | ✓ `oversized_tee` *sub*, ✓ `trousers`, ✓ `sneakers`, ✗ braided bun |
| mum | adult F | Needs garments | ✗ cardigan, ✗ patterned blouse, ✓ `trousers`, ✓ boots, ✗ wavy grey hair |
| newsboy_1900s | child M | Under-12 | ✗ braces, ✗ long-sleeve shirt, ✗ knickerbockers, ✗ flat cap, ✓ boots |
| oldtimer_1920s | senior M | Needs garments | ✗ three-piece tweed suit (✓ `suit_jacket` + `suit_trousers` *sub*), ✓ tie, ✓ `mustache`, ✓ `oxfords` |
| overalls_woman | adult F | Needs garments | ✗ dungarees, ✓ tee, ✓ `sneakers`, ✗ pixie cut |
| polo_man | adult M | **Approx. now** | ✗ polo (✓ `camp_shirt` *sub*), ✗ cargo trousers (✓ `trousers` *sub*), ✓ `belt`, ✓ boots, ✓ `mustache`, ✓ `buzz` |
| preschool_boy | 3–5 M | Under-12 | ✗ sweatshirt, ✗ joggers, ✓ `sneakers`, ✗ kids' bowl cut |
| preschool_girl | 3–5 F | Under-12 | ✗ pinafore dress, ✗ striped top, ✗ leggings, ✓ `sneakers`, ✗ afro puffs |
| preteen_boy | 11–12 M | Needs garments | ✗ hoodie, ✓ `cargo_shorts`, ✓ `sneakers`, ✗ curly top hair (age 12 works) |
| preteen_girl | 11–12 F | Needs garments | ✗ plaid shirt over tee, ✓ jeans, ✓ boots, ✗ long side braid |
| retired_man | senior M | Needs garments | ✗ cardigan over print shirt, ✓ `trousers`, ✓ `loafers`, ✓ `short01` |
| retired_woman | senior F | Needs garments | ✗ tunic blouse, ✗ capri trousers, ✗ glasses, ✗ short grey curls |
| rosa | senior F | Needs garments | ✗ crew sweater, ✓ `trousers`, ✓ `sneakers`, ✗ glasses, ✗ grey curly hair |
| school_boy | 6–8 M | Under-12 | ✗ puffer vest, ✗ long-sleeve top, ✗ joggers, ✗ glasses, ✓ `sneakers`, ✗ curly hair |
| school_girl | 6–8 F | Under-12 | ✗ cardigan + school blouse, ✗ pleated skirt, ✗ Mary Janes, ✓ `backpack`, ✗ pigtails |
| scientist_female | adult F | Needs garments | ✗ lab coat, ✓ `uniform_shirt_long` *sub*, ✓ `trousers`, ✓ `goggles`, ✓ `ponytail01` |
| scientist_male | adult M | Needs garments | ✗ lab coat, ✗ sweater, ✗ glasses, ✓ `trousers`, ✓ boots |
| senior_gent | senior M | Needs garments | ✗ sweater vest over long-sleeve shirt, ✓ `trousers`, ✓ shoes, ✗ swept white hair |
| senior_lady | senior F | Needs garments | ✗ cardigan, ✗ floral dress, ✓ shoes, ✗ grey bun |
| shopkeeper_1890s | adult M | Needs garments | ✗ waistcoat with watch chain (✓ `gold_chains` *sub*), ✗ long-sleeve shirt, ✓ tie, ✓ `trousers`, ✓ `mustache` |
| student | adult M | Needs garments | ✗ open long-sleeve shirt, ✓ `trousers`, ✓ shoes, ✓ `short02` |
| teen | teen F | Needs garments | ✗ zip hoodie, ✓ tee, ✓ `trousers`, ✓ `sneakers`, ✓ `bob02` |
| teen_bomber | teen F | Needs garments | ✗ bomber jacket, ✓ tee, ✗ cargo pants, ✓ `sneakers`, ✗ undercut |
| teen_boy_1950s | teen M | Needs garments | ✗ varsity jacket, ✓ tee, ✓ jeans (cuffed ✗), ✗ canvas high-tops, ✗ pompadour |
| teen_boy_1980s | teen M | Needs garments | ✗ denim jacket, ✗ striped tee, ✓ jeans, ✗ high-tops, ✗ mullet |
| teen_girl | teen F | Needs garments | ✗ zip hoodie, ✓ tee, ✓ jeans, ✓ `sneakers`, ✓ `bob01` |
| teen_girl_1950s | teen F | Needs garments | ✗ cardigan, ✗ swing skirt, ✗ saddle shoes, ✗ ponytail with scarf |
| teen_girl_1980s | teen F | Needs garments | ✗ colour-block windbreaker, ✓ jeans, ✓ `sneakers`, ✗ side ponytail with scrunchie |
| teen_sweater | teen M | Needs garments | ✗ crew sweater, ✓ jeans, ✗ high-tops, ✓ `short03` *sub* |
| werewolf_female | creature | Creature | non-human; keep Meshy |
| werewolf_male | creature | Creature | non-human; keep Meshy |
| woman_1910s | adult F | Needs garments | ✗ high-neck blouse, ✗ long skirt, ✓ boots, ✗ updo |
| woman_1930s | adult F | Needs garments | ✗ short-sleeve blouse, ✗ A-line midi skirt, ✓ shoes, ✗ 1930s waves |
| woman_1940s | adult F | Needs garments | ✗ shirt-dress, ✓ `belt`, ✓ shoes, ✗ victory rolls / head bow |
| woman_1950s | adult F | Needs garments | ✗ swing dress, ✗ bolero/cardigan, ✓ `belt`, ✓ shoes, ✗ curls |
| woman_1960s | adult F | Needs garments | ✗ mod shift dress with collar, ✗ white boots, ✓ `afro01` |
| woman_1970s | adult F | Needs garments | ✗ fitted knit top, ✗ flares, ✓ `belt`, ✓ shoes, ✗ long wavy hair |
| woman_1980s | adult F | Needs garments | ✗ blazer with shoulder pads (✓ `suit_jacket` *sub*), ✗ blouse, ✓ `trousers`, ✗ big curly hair |
| woman_1990s | adult F | Needs garments | ✗ plaid shirt over tee, ✓ jeans, ✓ boots, ✗ short tousled hair |
| woman_2000s | adult F | Needs garments | ✗ cropped denim jacket, ✗ cami, ✓ jeans, ✓ `box_braids` |
| young_man | adult M | Needs garments | ✗ overshirt, ✓ tee, ✓ jeans, ✓ boots, ✗ short curly hair |

## Suggested order

1. ~~Pipeline~~ (done, see above). Stylised faces done too. Next there:
   register generic characters in the game's manifest and model tables.
2. **Wardrobe library → fitted garments** for the most-shared pieces: knitwear,
   long-sleeve shirts, dresses and skirts, jackets, dungarees, waistcoats.
3. **Hair library**: the 10 MakeHuman styles now have cartoon shells; the cast still
   needs more styles (buns, braids, afros, curls, beehives), plus **glasses**.
4. **Under-12 age range** with cartoon child tuning.
5. Fantasy gear (alchemists). Creatures stay as Meshy models.
