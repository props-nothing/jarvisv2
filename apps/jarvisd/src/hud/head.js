// The head of the console: a real human face mesh drawn as a hologram.
//
// The geometry is MediaPipe's canonical face model (468 vertices, 898 triangles; Apache License 2.0, Copyright 2019-2023
// The MediaPipe Authors; github.com/google-ai-edge/mediapipe, mediapipe/modules/face_geometry/data/canonical_face_model.obj).
// Only positions and triangle indices are kept, as integers in hundredths of a model unit. The cranium and neck are
// drawn here as wire, the jaw and brows are rigged by weights computed from the geometry itself, and everything is
// plain 2D canvas: no library, no network, no texture. See docs/research/integrations/mediapipe-canonical-face-model.md.
//
// Model space: +x to the viewer's right, +y up, +z out of the face; the chin is about y = -9.4, the crown of the
// forehead about y = 8.3.
(function () {
  "use strict";

  var MESH = {"v":[0,-341,598,0,-113,748,0,-209,606,-46,96,663,0,-46,759,0,37,724,0,247,579,-425,258,328,0,402,528,0,489,539,0,826,448,0,-371,586,0,-392,557,0,-399,522,0,-454,540,0,-475,553,0,-502,560,0,-537,554,0,-615,507,0,-150,711,-42,-147,645,-709,543,10,-263,204,385,-320,199,380,-378,204,365,-447,242,316,-216,219,385,-321,322,412,-267,321,409,-375,317,397,-416,306,372,-506,193,278,-227,-743,439,-445,266,317,-721,226,7,-580,235,220,-284,-72,443,-71,-333,588,-61,-392,544,-143,-350,550,-191,-380,503,-113,-397,519,-156,-408,484,-265,-500,419,-43,-109,736,-50,-48,744,-525,388,336,-172,97,456,-161,-94,581,-165,-61,558,-477,-70,353,-48,30,710,-373,451,455,-459,430,405,-628,662,143,-122,414,511,-219,310,400,-310,-435,410,-672,-479,-175,-119,-131,574,-73,-159,583,-246,-434,428,-220,-430,416,-499,480,375,-159,-126,546,-264,452,492,-276,510,502,-352,801,373,-560,572,272,-306,657,453,-572,425,283,-637,479,159,-67,-369,574,-126,-379,542,-173,-395,500,-104,-146,566,-232,-433,426,-206,-448,452,-215,-428,404,-95,-104,651,-147,-404,460,-102,-399,493,-53,-399,514,-77,-610,499,-70,-529,545,-67,-495,551,-63,-470,545,-58,-452,534,-154,-442,475,-162,-448,481,-173,-462,485,-184,-483,482,-237,-311,487,-754,-105,-243,0,-172,660,-183,-440,440,-193,-441,450,-60,-201,587,-141,-171,524,-66,-182,586,-234,57,429,-333,10,411,-173,-92,527,-513,749,266,-454,632,368,-399,511,447,-217,-544,446,-140,501,532,-162,660,492,-189,824,427,-420,224,338,-573,141,243,-186,236,384,-499,307,308,-130,142,483,-131,-67,642,-647,94,169,-526,95,297,-443,72,352,-330,86,387,-243,113,404,-182,147,422,-56,231,557,-634,-53,188,-559,321,269,-24,-146,707,-161,34,490,-774,236,-201,-139,185,445,-179,-98,485,-467,266,308,-133,-28,610,-727,-289,-225,-186,259,376,-92,7,667,-500,-614,189,-509,-718,71,-716,-81,-7,-584,-525,92,-685,366,72,-241,-826,412,-18,-169,657,-210,-16,457,-641,224,156,-367,236,364,-318,229,378,-220,-460,448,-623,-194,166,-129,-930,409,-321,-853,280,-407,-799,193,0,655,503,0,-940,426,-272,232,378,-229,240,370,-200,250,369,-613,340,204,-229,289,378,-272,296,387,-318,296,388,-367,293,372,-402,286,348,-756,411,-99,-402,248,344,0,-252,593,-178,-268,521,-122,-118,595,-73,-254,582,0,327,524,-414,-700,267,-331,-766,338,-131,-864,470,-594,-622,-63,-200,274,374,-90,124,575,0,-877,489,-231,-897,361,-695,-244,-13,-110,-446,512,-118,-458,519,-126,-479,524,-133,-511,521,-155,-582,476,-195,-418,443,-212,-414,456,-229,-405,458,-285,-367,448,-528,-224,286,-95,191,520,-131,310,423,-178,286,388,-185,-410,425,-544,-403,211,-77,318,486,-194,-661,452,0,106,677,-52,158,615,0,173,632,-125,23,568,0,-794,518,0,-699,515,-100,-693,498,-329,-538,380,-231,-157,459,-268,-611,410,-383,-154,414,-296,-227,444,-439,-268,364,-122,-783,497,-154,-14,520,-388,-604,331,-308,-681,381,-375,-450,373,-609,-321,147,-459,-473,298,-658,-394,7,-349,-320,413,-126,80,531,-113,-93,654,-144,-114,591,-92,-53,700,-176,353,433,-263,371,436,-339,372,431,-408,368,408,-462,347,365,-517,254,267,-730,76,-5,-471,165,311,-407,148,348,-327,147,373,-253,162,387,-197,186,396,-158,210,408,-766,67,-244,-140,-134,563,-88,66,623,-77,-97,708,-46,-133,679,-75,-107,680,-124,-159,548,-39,-141,696,-32,-161,651,-164,256,386,-126,247,420,-103,238,462,-425,277,332,-453,291,334,46,96,663,425,258,328,42,-147,645,709,543,10,263,204,385,320,199,380,378,204,365,447,242,316,216,219,385,321,322,412,267,321,409,375,317,397,416,306,372,506,193,278,227,-743,439,445,266,317,721,226,7,580,235,220,284,-72,443,71,-333,588,61,-392,544,143,-350,550,191,-380,503,113,-397,519,156,-408,484,265,-500,419,43,-109,736,50,-48,744,525,388,336,172,97,456,161,-94,581,165,-61,558,477,-70,353,48,30,710,373,451,455,459,430,405,628,662,143,122,414,511,219,310,400,310,-435,410,672,-479,-175,119,-131,574,73,-159,583,246,-434,428,220,-430,416,499,480,375,159,-126,546,264,452,492,276,510,502,352,801,373,560,572,272,306,657,453,572,425,283,637,479,159,67,-369,574,126,-379,542,173,-395,500,104,-146,566,232,-433,426,206,-448,452,215,-428,404,95,-104,651,147,-404,460,102,-399,493,53,-399,514,77,-610,499,70,-529,545,67,-495,551,63,-470,545,58,-452,534,154,-442,475,162,-448,481,173,-462,485,184,-483,482,237,-311,487,754,-105,-243,183,-440,440,193,-441,450,60,-201,587,141,-171,524,66,-182,586,234,57,429,333,10,411,173,-92,527,513,749,266,454,632,368,399,511,447,217,-544,446,140,501,532,162,660,492,189,824,427,420,224,338,573,141,243,186,236,384,499,307,308,130,142,483,131,-67,642,647,94,169,526,95,297,443,72,352,330,86,387,243,113,404,182,147,422,56,231,557,634,-53,188,559,321,269,24,-146,707,161,34,490,774,236,-201,139,185,445,179,-98,485,467,266,308,133,-28,610,727,-289,-225,186,259,376,92,7,667,500,-614,189,509,-718,71,716,-81,-7,584,-525,92,685,366,72,241,-826,412,18,-169,657,210,-16,457,641,224,156,367,236,364,318,229,378,220,-460,448,623,-194,166,129,-930,409,321,-853,280,407,-799,193,272,232,378,229,240,370,200,250,369,613,340,204,229,289,378,272,296,387,318,296,388,367,293,372,402,286,348,756,411,-99,402,248,344,178,-268,521,122,-118,595,73,-254,582,414,-700,267,331,-766,338,131,-864,470,594,-622,-63,200,274,374,90,124,575,231,-897,361,695,-244,-13,110,-446,512,118,-458,519,126,-479,524,133,-511,521,155,-582,476,195,-418,443,212,-414,456,229,-405,458,285,-367,448,528,-224,286,95,191,520,131,310,423,178,286,388,185,-410,425,544,-403,211,77,318,486,194,-661,452,52,158,615,125,23,568,100,-693,498,329,-538,380,231,-157,459,268,-611,410,383,-154,414,296,-227,444,439,-268,364,122,-783,497,154,-14,520,388,-604,331,308,-681,381,375,-450,373,609,-321,147,459,-473,298,658,-394,7,349,-320,413,126,80,531,113,-93,654,144,-114,591,92,-53,700,176,353,433,263,371,436,339,372,431,408,368,408,462,347,365,517,254,267,730,76,-5,471,165,311,407,148,348,327,147,373,253,162,387,197,186,396,158,210,408,766,67,-244,140,-134,563,88,66,623,77,-97,708,46,-133,679,75,-107,680,124,-159,548,39,-141,696,32,-161,651,164,256,386,126,247,420,103,238,462,425,277,332,453,291,334],"f":[173,155,133,246,33,7,382,398,362,263,466,249,308,415,324,78,95,191,356,389,264,127,34,162,368,264,389,139,162,34,267,0,302,37,72,0,11,302,0,11,0,72,349,451,350,120,121,231,452,350,451,232,231,121,267,302,269,37,39,72,303,269,302,73,72,39,357,343,350,128,121,114,277,350,343,47,114,121,350,452,357,121,128,232,453,357,452,233,232,128,299,333,297,69,67,104,332,297,333,103,104,67,175,152,396,175,171,152,377,396,152,148,152,171,381,384,382,154,155,157,398,382,384,173,157,155,280,347,330,50,101,118,348,330,347,119,118,101,269,303,270,39,40,73,304,270,303,74,73,40,9,336,151,9,151,107,337,151,336,108,107,151,344,278,360,115,131,48,279,360,278,49,48,131,262,431,418,32,194,211,424,418,431,204,211,194,304,408,270,74,40,184,409,270,408,185,184,40,272,310,407,42,183,80,415,407,310,191,80,183,322,270,410,92,186,40,409,410,270,185,40,186,347,449,348,118,119,229,450,348,449,230,229,119,434,432,430,214,210,212,422,430,432,202,212,210,313,314,18,83,18,84,17,18,314,17,84,18,307,375,306,77,76,146,291,306,375,61,146,76,259,387,260,29,30,160,388,260,387,161,160,30,286,414,384,56,157,190,398,384,414,173,190,157,418,424,406,194,182,204,335,406,424,106,204,182,367,416,364,138,135,192,434,364,416,214,192,135,391,423,327,165,98,203,358,327,423,129,203,98,298,301,284,68,54,71,251,284,301,21,71,54,4,275,5,4,5,45,281,5,275,51,45,5,254,373,253,24,23,144,374,253,373,145,144,23,320,321,307,90,77,91,375,307,321,146,91,77,280,425,411,50,187,205,427,411,425,207,205,187,421,313,200,201,200,83,18,200,313,18,83,200,335,321,406,106,182,91,405,406,321,181,91,182,405,321,404,181,180,91,320,404,321,90,91,180,17,314,16,17,16,84,315,16,314,85,84,16,425,266,426,205,206,36,423,426,266,203,36,206,369,396,400,140,176,171,377,400,396,148,171,176,391,269,322,165,92,39,270,322,269,40,39,92,417,465,413,193,189,245,464,413,465,244,245,189,257,258,386,27,159,28,385,386,258,158,28,159,260,388,467,30,247,161,466,467,388,246,161,247,248,456,419,3,196,236,399,419,456,174,236,196,333,298,332,104,103,68,284,332,298,54,68,103,285,8,417,55,193,8,168,417,8,168,8,193,340,261,346,111,117,31,448,346,261,228,31,117,285,417,441,55,221,193,413,441,417,189,193,221,327,460,326,98,97,240,328,326,460,99,240,97,277,355,329,47,100,126,371,329,355,142,126,100,309,392,438,79,218,166,439,438,392,219,166,218,381,382,256,154,26,155,341,256,382,112,155,26,360,279,420,131,198,49,429,420,279,209,49,198,365,364,379,136,150,135,394,379,364,169,135,150,355,277,437,126,217,47,343,437,277,114,47,217,443,444,282,223,52,224,283,282,444,53,224,52,281,275,363,51,134,45,440,363,275,220,45,134,431,262,395,211,170,32,369,395,262,140,32,170,337,299,338,108,109,69,297,338,299,67,69,109,335,273,321,106,91,43,375,321,273,146,43,91,348,450,349,119,120,230,451,349,450,231,230,120,467,359,342,247,113,130,446,342,359,226,130,113,282,283,334,52,105,53,293,334,283,63,53,105,250,458,462,20,242,238,461,462,458,241,238,242,276,353,300,46,70,124,383,300,353,156,124,70,325,292,324,96,95,62,308,324,292,78,62,95,283,276,293,53,63,46,300,293,276,70,46,63,447,264,345,227,116,34,372,345,264,143,34,116,352,345,346,123,117,116,340,346,345,111,116,117,1,19,274,1,44,19,354,274,19,125,19,44,248,281,456,3,236,51,363,456,281,134,51,236,425,426,427,205,207,206,436,427,426,216,206,207,380,381,252,153,22,154,256,252,381,26,154,22,391,393,269,165,39,167,267,269,393,37,167,39,199,428,200,199,200,208,421,200,428,201,208,200,330,329,266,101,36,100,371,266,329,142,100,36,422,432,273,202,43,212,287,273,432,57,212,43,290,250,328,60,99,20,462,328,250,242,20,99,258,286,385,28,158,56,384,385,286,157,56,158,342,446,353,113,124,226,265,353,446,35,226,124,257,386,259,27,29,159,387,259,386,160,159,29,430,422,431,210,211,202,424,431,422,204,202,211,445,342,276,225,46,113,353,276,342,124,113,46,424,422,335,204,106,202,273,335,422,43,202,106,306,292,307,76,77,62,325,307,292,96,62,77,366,447,352,137,123,227,345,352,447,116,227,123,302,268,303,72,73,38,271,303,268,41,38,73,371,358,266,142,36,129,423,266,358,203,129,36,327,294,460,98,240,64,455,460,294,235,64,240,294,331,278,64,48,102,279,278,331,49,102,48,303,271,304,73,74,41,272,304,271,42,41,74,427,436,434,207,214,216,432,434,436,212,216,214,304,272,408,74,184,42,407,408,272,183,42,184,394,430,395,169,170,210,431,395,430,211,210,170,395,369,378,170,149,140,400,378,369,176,140,149,296,334,299,66,69,105,333,299,334,104,105,69,417,168,351,193,122,168,6,351,168,6,168,122,280,411,352,50,123,187,376,352,411,147,187,123,319,320,325,89,96,90,307,325,320,77,90,96,285,295,336,55,107,65,296,336,295,66,65,107,404,320,403,180,179,90,319,403,320,89,90,179,330,348,329,101,100,119,349,329,348,120,119,100,334,293,333,105,104,63,298,333,293,68,63,104,323,454,366,93,137,234,447,366,454,227,234,137,16,315,15,16,15,85,316,15,315,86,85,15,429,279,358,209,129,49,331,358,279,102,49,129,15,316,14,15,14,86,317,14,316,87,86,14,8,285,9,8,9,55,336,9,285,107,55,9,329,349,277,100,47,120,350,277,349,121,120,47,252,253,380,22,153,23,374,380,253,145,23,153,402,403,318,178,88,179,319,318,403,89,179,88,351,6,419,122,196,6,197,419,6,197,6,196,324,318,325,95,96,88,319,325,318,89,88,96,397,367,365,172,136,138,364,365,367,135,138,136,288,435,397,58,172,215,367,397,435,138,215,172,438,439,344,218,115,219,278,344,439,48,219,115,271,311,272,41,42,81,310,272,311,80,81,42,5,281,195,5,195,51,248,195,281,3,51,195,273,287,375,43,146,57,291,375,287,61,57,146,396,428,175,171,175,208,199,175,428,199,208,175,268,312,271,38,41,82,311,271,312,81,82,41,444,445,283,224,53,225,276,283,445,46,225,53,254,339,373,24,144,110,390,373,339,163,110,144,295,282,296,65,66,52,334,296,282,105,52,66,346,448,347,117,118,228,449,347,448,229,228,118,454,356,447,234,227,127,264,447,356,34,127,227,336,296,337,107,108,66,299,337,296,69,66,108,151,337,10,151,10,108,338,10,337,109,108,10,278,439,294,48,64,219,455,294,439,235,219,64,407,415,292,183,62,191,308,292,415,78,191,62,358,371,429,129,209,142,355,429,371,126,142,209,345,372,340,116,111,143,265,340,372,35,143,111,388,390,466,161,246,163,249,466,390,7,163,246,352,346,280,123,50,117,347,280,346,118,117,50,295,442,282,65,52,222,443,282,442,223,222,52,19,94,354,19,125,94,370,354,94,141,94,125,295,285,442,65,222,55,441,442,285,221,55,222,419,197,248,196,3,197,195,248,197,195,197,3,359,263,255,130,25,33,249,255,263,7,33,25,275,274,440,45,220,44,457,440,274,237,44,220,300,383,301,70,71,156,368,301,383,139,156,71,417,351,465,193,245,122,412,465,351,188,122,245,466,263,467,246,247,33,359,467,263,130,33,247,389,251,368,162,139,21,301,368,251,71,21,139,374,386,380,145,153,159,385,380,386,158,159,153,379,394,378,150,149,169,395,378,394,170,169,149,351,419,412,122,188,196,399,412,419,174,196,188,426,322,436,206,216,92,410,436,322,186,92,216,387,373,388,160,161,144,390,388,373,163,144,161,393,326,164,167,164,97,2,164,326,2,97,164,354,370,461,125,241,141,462,461,370,242,141,241,0,267,164,0,164,37,393,164,267,167,37,164,11,12,302,11,72,12,268,302,12,38,12,72,386,374,387,159,160,145,373,387,374,144,145,160,12,13,268,12,38,13,312,268,13,82,13,38,293,300,298,63,68,70,301,298,300,71,70,68,340,265,261,111,31,35,446,261,265,226,35,31,380,385,381,153,154,158,384,381,385,157,158,154,280,330,425,50,205,101,266,425,330,36,101,205,423,391,426,203,206,165,322,426,391,92,165,206,429,355,420,209,198,126,437,420,355,217,126,198,391,327,393,165,167,98,326,393,327,97,98,167,457,438,440,237,220,218,344,440,438,115,218,220,382,362,341,155,112,133,463,341,362,243,133,112,457,461,459,237,239,241,458,459,461,238,241,239,434,430,364,214,135,210,394,364,430,169,210,135,414,463,398,190,173,243,362,398,463,133,243,173,262,428,369,32,140,208,396,369,428,171,208,140,457,274,461,237,241,44,354,461,274,125,44,241,316,403,317,86,87,179,402,317,403,178,179,87,315,404,316,85,86,180,403,316,404,179,180,86,314,405,315,84,85,181,404,315,405,180,181,85,313,406,314,83,84,182,405,314,406,181,182,84,418,406,421,194,201,182,313,421,406,83,182,201,366,401,323,137,93,177,361,323,401,132,177,93,408,407,306,184,76,183,292,306,407,62,183,76,408,306,409,184,185,76,291,409,306,61,76,185,410,409,287,186,57,185,291,287,409,61,185,57,436,410,432,216,212,186,287,432,410,57,186,212,434,416,427,214,207,192,411,427,416,187,192,207,264,368,372,34,143,139,383,372,368,156,139,143,457,459,438,237,218,239,309,438,459,79,239,218,352,376,366,123,137,147,401,366,376,177,147,137,4,1,275,4,45,1,274,275,1,44,1,45,428,262,421,208,201,32,418,421,262,194,32,201,327,358,294,98,64,129,331,294,358,102,129,64,367,435,416,138,192,215,433,416,435,213,215,192,455,439,289,235,59,219,392,289,439,166,219,59,328,462,326,99,97,242,370,326,462,141,242,97,326,370,2,97,2,141,94,2,370,94,141,2,460,455,305,240,75,235,289,305,455,59,235,75,448,339,449,228,229,110,254,449,339,24,110,229,261,446,255,31,25,226,359,255,446,130,226,25,449,254,450,229,230,24,253,450,254,23,24,230,450,253,451,230,231,23,252,451,253,22,23,231,451,252,452,231,232,22,256,452,252,26,22,232,256,341,452,26,232,112,453,452,341,233,112,232,413,464,414,189,190,244,463,414,464,243,244,190,441,413,286,221,56,189,414,286,413,190,189,56,441,286,442,221,222,56,258,442,286,28,56,222,442,258,443,222,223,28,257,443,258,27,28,223,444,443,259,224,29,223,257,259,443,27,223,29,259,260,444,29,224,30,445,444,260,225,30,224,260,467,445,30,225,247,342,445,467,113,247,225,250,309,458,20,238,79,459,458,309,239,79,238,290,305,392,60,166,75,289,392,305,59,75,166,460,305,328,240,99,75,290,328,305,60,75,99,376,433,401,147,177,213,435,401,433,215,213,177,250,290,309,20,79,60,392,309,290,166,60,79,411,416,376,187,147,192,433,376,416,213,192,147,341,463,453,112,233,243,464,453,463,244,243,233,453,464,357,233,128,244,465,357,464,245,244,128,412,343,465,188,245,114,357,465,343,128,114,245,437,343,399,217,174,114,412,399,343,188,114,174,363,440,360,134,131,220,344,360,440,115,220,131,456,420,399,236,174,198,437,399,420,217,198,174,456,363,420,236,198,134,360,420,363,131,134,198,361,401,288,132,58,177,435,288,401,215,177,58,353,265,383,124,156,35,372,383,265,143,35,156,255,249,339,25,110,7,390,339,249,163,7,110,261,255,448,31,228,25,339,448,255,110,25,228,14,317,13,14,13,87,312,13,317,82,87,13,317,402,312,87,82,178,311,312,402,81,178,82,402,318,311,178,81,88,310,311,318,80,88,81,318,324,310,88,80,95,415,310,324,191,95,80]};

  var N = MESH.v.length / 3;
  var BASE = new Float32Array(N * 3);
  var i, j;
  for (i = 0; i < BASE.length; i += 1) BASE[i] = MESH.v[i] / 100;
  var TRI = MESH.f;
  var TRIS = TRI.length / 3;

  // Landmark rings (indices into the canonical mesh), checked against the geometry when this file was written.
  var EYE_L = [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246];
  var EYE_R = [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466];
  var BROW_L = [70, 63, 105, 66, 107];
  var BROW_R = [300, 293, 334, 296, 336];
  var LIPS_OUT = [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185];
  var LIPS_IN = [78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308, 415, 310, 311, 312, 13, 82, 81, 80, 191];
  var NOSE = [168, 6, 197, 195, 5, 4, 1];

  function smooth(a, b, x) {
    var t = Math.max(0, Math.min(1, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
  }

  // Jaw weights: everything below the line where the lips meet follows the jaw, less so towards the cheeks, so the
  // mouth opens and the chin drops without tearing the face. Brow weights fall off with distance from the brow landmarks.
  var LIP_LINE = -4.2;
  var jaw = new Float32Array(N);
  var brow = new Float32Array(N);
  for (i = 0; i < N; i += 1) {
    var x = BASE[i * 3], y = BASE[i * 3 + 1];
    jaw[i] = smooth(0, 1, (LIP_LINE - y) / 0.55) * (1 - 0.55 * smooth(4.2, 7.6, Math.abs(x)));
    var best = 99;
    for (j = 0; j < BROW_L.length; j += 1) {
      var bl = BROW_L[j] * 3, br = BROW_R[j] * 3;
      best = Math.min(best,
        Math.hypot(x - BASE[bl], y - BASE[bl + 1], BASE[i * 3 + 2] - BASE[bl + 2]),
        Math.hypot(x - BASE[br], y - BASE[br + 1], BASE[i * 3 + 2] - BASE[br + 2]));
    }
    brow[i] = Math.max(0, 1 - best / 2.4);
  }
  // The corners of the mouth are pinned part-way, so the lips shear rather than split.
  jaw[61] = Math.max(jaw[61], 0.3); jaw[291] = Math.max(jaw[291], 0.3);

  // Expression weights, found from the geometry in the same way. The eyes squash and stretch about their own centres (which is
  // what a lid does to a wire face), the corners of the mouth lift or drop with a smile or a frown, and the inner and outer ends
  // of each brow move on their own, which is what tells worry from anger from a raised eyebrow.
  function centre(ids) {
    var c = [0, 0], k;
    for (k = 0; k < ids.length; k += 1) { c[0] += BASE[ids[k] * 3]; c[1] += BASE[ids[k] * 3 + 1]; }
    return [c[0] / ids.length, c[1] / ids.length];
  }
  var MOUTH_C = centre(LIPS_OUT), EYE_C = [centre(EYE_L), centre(EYE_R)];
  var mouthW = new Float32Array(N), eyeW = new Float32Array(N), eyeSide = new Uint8Array(N);
  var innerW = new Float32Array(N), sideW = new Float32Array(N);
  for (i = 0; i < N; i += 1) {
    var vx = BASE[i * 3], vy = BASE[i * 3 + 1], e;
    mouthW[i] = smooth(0, 1, 1 - Math.hypot((vx - MOUTH_C[0]) / 5.4, (vy - MOUTH_C[1]) / 2.7));
    for (e = 0; e < 2; e += 1) {
      var w = smooth(0, 1, 1 - Math.hypot((vx - EYE_C[e][0]) / 2.5, (vy - EYE_C[e][1]) / 1.35));
      if (w > eyeW[i]) { eyeW[i] = w; eyeSide[i] = e; }
    }
    innerW[i] = brow[i] * (1 - smooth(1.0, 3.4, Math.abs(vx)));
    sideW[i] = brow[i] * (vx > 0 ? 1 : -1);
  }

  var cur = new Float32Array(N * 3);   // deformed model space
  var sx = new Float32Array(N), sy = new Float32Array(N), sz = new Float32Array(N);
  var rx = new Float32Array(N), ry = new Float32Array(N);

  // A skull and neck, as wire. The mesh is a mask; these close the shape behind it.
  var SKULL_C = [0, 1.6, -1.2], SKULL_R = [7.7, 10.4, 9.2];
  var skull = [];
  (function () {
    var lat, lon, ring, pts;
    for (lat = -2; lat <= 9; lat += 1) {
      var a = (lat / 10) * (Math.PI / 2) * 1.0;
      ring = [];
      for (lon = 0; lon <= 36; lon += 1) {
        var b = (lon / 36) * Math.PI * 2;
        ring.push([SKULL_C[0] + SKULL_R[0] * Math.cos(a) * Math.sin(b), SKULL_C[1] + SKULL_R[1] * Math.sin(a), SKULL_C[2] + SKULL_R[2] * Math.cos(a) * Math.cos(b)]);
      }
      skull.push(ring);
    }
    for (lon = 0; lon < 12; lon += 1) {
      var bb = (lon / 12) * Math.PI * 2;
      pts = [];
      for (lat = -2; lat <= 20; lat += 1) {
        var aa = (lat / 20) * (Math.PI / 2);
        pts.push([SKULL_C[0] + SKULL_R[0] * Math.cos(aa) * Math.sin(bb), SKULL_C[1] + SKULL_R[1] * Math.sin(aa), SKULL_C[2] + SKULL_R[2] * Math.cos(aa) * Math.cos(bb)]);
      }
      skull.push(pts);
    }
  })();
  var neck = [];
  (function () {
    var k, s, ring;
    for (k = 0; k < 9; k += 1) {
      var yy = -8.2 - k * 1.35, rr = 4.5 + Math.min(k, 3) * 0.25;
      ring = [];
      for (s = 0; s <= 28; s += 1) {
        var b = (s / 28) * Math.PI * 2;
        ring.push([Math.sin(b) * rr, yy, -2.2 + Math.cos(b) * rr * 0.9]);
      }
      neck.push(ring);
    }
  })();

  var frontFacing = 1;   // sign of the triangle normal that faces the viewer, found once from the rest pose
  var pose = { yaw: 0, pitch: 0, roll: 0, jaw: 0, blink: 0, eyeOpen: 1, look: [0, 0] };
  // What the face is doing, as a handful of channels that ease towards whatever the mind below wants.
  var expr = { brow: 0, inner: 0, asym: 0, smile: 0.05, open: 1, jaw: 0, yaw: 0, pitch: 0, roll: 0 };

  // Expressions, as targets for those channels. brow lifts both brows, inner lifts (or, negative, drops) the inner ends (worry
  // up, anger down), asym lifts one brow more than the other (a raised eyebrow), smile is a smile (or, negative, a frown), open is
  // how wide the eyes are, jaw is how far the mouth hangs, and yaw/pitch/roll lean the head.
  var EMO = {
    neutral:   { brow: 0,     inner: 0,     asym: 0,    smile: 0.06,  open: 1,    jaw: 0,    yaw: 0,     pitch: 0,     roll: 0 },
    pleasant:  { brow: 0.08,  inner: 0,     asym: 0,    smile: 0.28,  open: 0.96, jaw: 0,    yaw: 0,     pitch: -0.01,  roll: 0.01 },
    attentive: { brow: 0.5,   inner: 0.15,  asym: 0,    smile: 0.12,  open: 1.16, jaw: 0,    yaw: 0,     pitch: -0.04,  roll: 0 },
    curious:   { brow: 0.35,  inner: 0.1,   asym: 0.85, smile: 0.1,   open: 1.08, jaw: 0,    yaw: 0.08,  pitch: -0.02,  roll: 0.1 },
    amused:    { brow: 0.3,   inner: -0.1,  asym: 0.3,  smile: 1,     open: 0.68, jaw: 0.03, yaw: -0.05, pitch: -0.03,  roll: -0.06 },
    pleased:   { brow: 0.3,   inner: 0,     asym: 0,    smile: 0.72,  open: 0.84, jaw: 0,    yaw: 0,     pitch: -0.035, roll: 0.02 },
    thinking:  { brow: 0.12,  inner: 0.3,   asym: 0.55, smile: -0.05, open: 0.84, jaw: 0,    yaw: 0.14,  pitch: 0.05,   roll: -0.04 },
    focused:   { brow: -0.3,  inner: -0.45, asym: 0,    smile: -0.02, open: 0.74, jaw: 0,    yaw: 0,     pitch: 0.02,   roll: 0 },
    alert:     { brow: 0.9,   inner: 0.25,  asym: 0,    smile: 0,     open: 1.3,  jaw: 0.04, yaw: 0,     pitch: -0.05,  roll: 0 },
    surprised: { brow: 1,     inner: 0.3,   asym: 0,    smile: 0.05,  open: 1.45, jaw: 0.3,  yaw: 0,     pitch: -0.07,  roll: 0 },
    concerned: { brow: 0.15,  inner: 0.95,  asym: 0,    smile: -0.6,  open: 0.94, jaw: 0,    yaw: 0,     pitch: 0.07,   roll: 0.05 },
    sleepy:    { brow: -0.12, inner: 0,     asym: 0,    smile: 0.02,  open: 0.3,  jaw: 0,    yaw: 0,     pitch: 0.13,   roll: 0.02 }
  };
  var CHANNELS = ["brow", "inner", "asym", "smile", "open", "jaw", "yaw", "pitch", "roll"];

  // The mind: nothing here follows the pointer. When nothing is happening the face lives on its own: it looks about, changes
  // expression, blinks, sighs and nods off after a long quiet; when something is happening it reacts to that (listening,
  // thinking, speaking, waiting on you), and the console can make it react to an event (emote, nod, shake).
  var IDLE_MOODS = [["neutral", 5], ["pleasant", 3], ["curious", 2], ["thinking", 1.6], ["amused", 1.1], ["pleased", 1], ["focused", 0.8], ["attentive", 0.8]];
  var mind = {
    emotion: "neutral", until: 0, gazeAt: 0, gazeTo: [0, 0], look: [0, 0], jitterAt: 0, jitter: [0, 0],
    blinkAt: 2.5, blinkFrom: -9, blinkLen: 0.17, lastT: 0, lastMode: "idle", quietFrom: 0,
    override: null, requested: null, speakMood: "pleasant", nodFrom: -9, shakeFrom: -9, sighFrom: -9, nextSigh: 25,
    beatAt: 0, kick: 0, lastMouth: 0
  };
  function pick(table) {
    var total = 0, k;
    for (k = 0; k < table.length; k += 1) total += table[k][1];
    var r = Math.random() * total;
    for (k = 0; k < table.length; k += 1) { r -= table[k][1]; if (r <= 0) return table[k][0]; }
    return table[0][0];
  }
  function between(a, b) { return a + Math.random() * (b - a); }

  function rotate(px, py, pz, yaw, pitch, roll, out) {
    var cy = Math.cos(yaw), sy2 = Math.sin(yaw), cp = Math.cos(pitch), sp = Math.sin(pitch), cr = Math.cos(roll), sr = Math.sin(roll);
    var x1 = px * cy + pz * sy2, z1 = -px * sy2 + pz * cy;
    var y2 = py * cp - z1 * sp, z2 = py * sp + z1 * cp;
    out[0] = x1 * cr - y2 * sr; out[1] = x1 * sr + y2 * cr; out[2] = z2;
  }
  var tmp = [0, 0, 0];

  function deform() {
    var open = pose.jaw, ang = open * 0.2, ca = Math.cos(ang), sa = Math.sin(ang);
    var py = 0.4, pz = -2.4;
    for (var v = 0; v < N; v += 1) {
      var x = BASE[v * 3], y = BASE[v * 3 + 1], z = BASE[v * 3 + 2];
      var w = jaw[v] * (open > 0 ? 1 : 0);
      if (w > 0) {
        var dy = y - py, dz = z - pz;
        var ny = py + dy * ca - dz * sa, nz = pz + dy * sa + dz * ca;
        y += (ny - y) * w; z += (nz - z) * w;
        // the mouth also widens a little as it opens, which is what reads as speech rather than a hinge
        x *= 1 + 0.05 * open * w;
      }
      y += brow[v] * expr.brow * 0.9 + innerW[v] * expr.inner * 1.1 + sideW[v] * expr.asym * 0.8;
      // the eyes: squash or stretch about each eye's centre, which is a blink, a squint or wide eyes
      var ew = eyeW[v];
      if (ew > 0) {
        var ec = EYE_C[eyeSide[v]];
        y = ec[1] + (y - ec[1]) * (1 + (pose.eyeOpen - 1) * ew);
      }
      // the mouth: the corners lift with a smile and drop with a frown, and it widens a little with a smile
      var mw = mouthW[v];
      if (mw > 0) {
        var corner = smooth(0.6, 3.4, Math.abs(x - MOUTH_C[0]));
        y += expr.smile * (0.95 * corner - 0.12 * (1 - corner)) * mw;
        x += (x - MOUTH_C[0]) * 0.07 * expr.smile * mw;
      }
      cur[v * 3] = x; cur[v * 3 + 1] = y; cur[v * 3 + 2] = z;
    }
  }

  function project(cx, cy, scale) {
    var D = 70;
    for (var v = 0; v < N; v += 1) {
      rotate(cur[v * 3], cur[v * 3 + 1] - 1.2, cur[v * 3 + 2] + 1.2, pose.yaw, pose.pitch, pose.roll, tmp);
      var f = D / (D - tmp[2]);
      sx[v] = tmp[0]; sy[v] = tmp[1]; sz[v] = tmp[2];
      rx[v] = cx + tmp[0] * scale * f; ry[v] = cy - tmp[1] * scale * f;
    }
  }

  function toScreen(p, cx, cy, scale, out) {
    rotate(p[0], p[1] - 1.2, p[2] + 1.2, pose.yaw, pose.pitch, pose.roll, tmp);
    var f = 70 / (70 - tmp[2]);
    out[0] = cx + tmp[0] * scale * f; out[1] = cy - tmp[1] * scale * f; out[2] = tmp[2];
  }

  function polyline(ctx, ring, cx, cy, scale, zMax, alphaOf, rgba) {
    var o = [0, 0, 0], drawing = false, prev = null;
    for (var k = 0; k < ring.length; k += 1) {
      var p = ring[k];
      if (p[2] > zMax) { drawing = false; continue; }
      toScreen(p, cx, cy, scale, o);
      if (!drawing) { ctx.beginPath(); ctx.moveTo(o[0], o[1]); drawing = true; } else ctx.lineTo(o[0], o[1]);
      if (k === ring.length - 1 || ring[k + 1][2] > zMax) { ctx.strokeStyle = rgba(alphaOf(o[2])); ctx.stroke(); drawing = false; }
    }
    return prev;
  }

  function strip(ctx, ids, close, width, alpha, rgba) {
    ctx.beginPath();
    for (var k = 0; k < ids.length; k += 1) {
      var id = ids[k];
      if (k === 0) ctx.moveTo(rx[id], ry[id]); else ctx.lineTo(rx[id], ry[id]);
    }
    if (close) ctx.closePath();
    ctx.lineWidth = width; ctx.strokeStyle = rgba(alpha); ctx.stroke();
  }

  function eye(ctx, ids, side, scale, state, rgba) {
    var ex = 0, ey = 0, k;
    for (k = 0; k < ids.length; k += 1) { ex += rx[ids[k]]; ey += ry[ids[k]]; }
    ex /= ids.length; ey /= ids.length;
    var open = Math.max(0, Math.min(1, pose.eyeOpen));
    var gx = pose.look[0] * scale * 0.55, gy = -pose.look[1] * scale * 0.3;
    // wider eyes glow larger and brighter, a squint or a blink flattens the glow, so the light reads as an eye and not a lamp
    var r = scale * (1.35 + 0.5 * Math.min(1.4, pose.eyeOpen));
    ctx.save();
    ctx.translate(ex + gx, ey + gy);
    ctx.scale(1, Math.max(0.1, Math.min(1.25, pose.eyeOpen)));
    var g = ctx.createRadialGradient(0, 0, 0, 0, 0, r);
    g.addColorStop(0, "rgba(255,255,255," + (0.95 * Math.min(1, open * 1.4)) + ")");
    g.addColorStop(0.25, rgba(0.85 * Math.min(1, open * 1.4)));
    g.addColorStop(1, rgba(0));
    ctx.fillStyle = g;
    ctx.beginPath(); ctx.arc(0, 0, r, 0, Math.PI * 2); ctx.fill();
    ctx.restore();
  }

  // Runs once a frame: chooses what the face is feeling, eases every channel towards it, and moves the eyes and head.
  function think(t, mode, state) {
    var dt = Math.max(0.001, Math.min(0.1, t - mind.lastT));
    mind.lastT = t;
    var sway = state.calm ? 0 : 1;

    // a request from the console (emote) starts now and runs for its duration
    if (mind.requested) { mind.override = { name: mind.requested.name, until: t + mind.requested.seconds }; mind.requested = null; }
    if (mode !== "idle") mind.quietFrom = t;
    if (mode !== mind.lastMode) { mind.until = 0; mind.gazeAt = 0; mind.lastMode = mode; }

    // 1. what it feels
    var emotion = mind.emotion;
    if (mind.override && t < mind.override.until) {
      emotion = mind.override.name;
    } else if (mode === "offline") {
      emotion = "sleepy";
    } else if (mode === "waiting") {
      emotion = Math.floor(t / 3.2) % 2 ? "alert" : "curious";
    } else if (mode === "listening") {
      emotion = "attentive";
    } else if (mode === "working") {
      if (t >= mind.until) { emotion = Math.random() < 0.55 ? "thinking" : "focused"; mind.until = t + between(2.2, 4.5); } else emotion = mind.emotion;
    } else if (mode === "speaking") {
      emotion = mind.speakMood;
    } else if (t >= mind.until) {
      // standing by: drift between moods, and nod off after a long quiet
      emotion = t - mind.quietFrom > 150 ? "sleepy" : pick(IDLE_MOODS);
      mind.until = t + between(3.5, 8.5);
    } else {
      emotion = mind.emotion;
    }
    mind.emotion = emotion;

    var target = EMO[emotion] || EMO.neutral;
    var rate = 1 - Math.exp(-dt * 4.2);
    var k;
    for (k = 0; k < CHANNELS.length; k += 1) {
      var key = CHANNELS[k], want = target[key];
      if (key === "brow") want += mind.kick;
      expr[key] += (want - expr[key]) * rate;
    }
    mind.kick *= Math.exp(-dt * 2.6);

    // speaking: a beat of the voice lifts the brows and nods the head, so the face emphasises what is said
    var mouth = Math.max(0, Math.min(1, state.mouth));
    if (mode === "speaking" && mouth > 0.7 && mind.lastMouth < 0.4 && t > mind.beatAt) {
      mind.beatAt = t + between(0.7, 1.4);
      mind.kick = 0.35;
      if (Math.random() < 0.3) mind.nodFrom = t;
    }
    mind.lastMouth = mouth;

    // 2. where it looks: eyes first, in quick jumps, and the head follows a little behind
    if (t >= mind.gazeAt) {
      var to = mind.gazeTo, hold;
      if (mode === "listening") { to = Math.random() < 0.8 ? [0, 0.02] : [between(-0.35, 0.35), between(-0.1, 0.2)]; hold = between(1.2, 3); }
      else if (mode === "speaking") { to = Math.random() < 0.7 ? [0, 0] : [between(-0.7, 0.7), between(0.1, 0.45)]; hold = between(1.4, 3.2); }
      else if (mode === "working") { to = [(Math.random() < 0.5 ? -1 : 1) * between(0.3, 0.8), between(0.15, 0.55)]; hold = between(0.9, 2.2); }
      else if (mode === "waiting") { to = Math.random() < 0.5 ? [0, 0] : [-0.75, -0.45]; hold = between(1, 2.2); }
      else if (mode === "offline") { to = [0, -0.3]; hold = 5; }
      else {
        var r = Math.random();
        if (emotion === "sleepy") { to = [0, -0.35]; hold = between(3, 6); }
        else if (r < 0.3) { to = [between(-0.05, 0.05), between(-0.05, 0.1)]; hold = between(1.5, 4); }
        else if (r < 0.4) { to = [0.95, -0.1]; hold = between(0.8, 1.6); }
        else { to = [between(-0.85, 0.85), between(-0.45, 0.55)]; hold = between(0.8, 3.2); }
      }
      if (Math.hypot(to[0] - mind.gazeTo[0], to[1] - mind.gazeTo[1]) > 0.7 && Math.random() < 0.45) mind.blinkAt = Math.min(mind.blinkAt, t + 0.08);
      mind.gazeTo = to;
      mind.gazeAt = t + hold;
    }
    if (t >= mind.jitterAt) { mind.jitter = [between(-0.03, 0.03), between(-0.03, 0.03)]; mind.jitterAt = t + between(0.25, 0.9); }
    var glide = 1 - Math.exp(-dt * 16);
    mind.look[0] += (mind.gazeTo[0] + mind.jitter[0] - mind.look[0]) * glide;
    mind.look[1] += (mind.gazeTo[1] + mind.jitter[1] - mind.look[1]) * glide;
    pose.look = mind.look;

    // 3. blinks: irregular, sometimes two together, slow when sleepy
    if (t >= mind.blinkAt) {
      mind.blinkFrom = t;
      mind.blinkLen = emotion === "sleepy" ? 0.4 : 0.17;
      mind.blinkAt = t + (emotion === "sleepy" ? between(1.5, 3) : Math.random() < 0.15 ? 0.3 : between(2, 6));
    }
    var into = (t - mind.blinkFrom) / mind.blinkLen;
    var blink = into >= 0 && into <= 1 ? Math.sin(Math.PI * into) : 0;
    if (mode === "offline") blink = 1;
    pose.blink = blink;
    pose.eyeOpen = Math.max(0.1, expr.open * (1 - 0.92 * blink));

    // 4. the head: a slow wander of its own, the lean of the emotion, a lean after the eyes, and any nod, shake or sigh
    var wander = sway * (mode === "speaking" ? 1.4 : 1);
    var yaw = expr.yaw + wander * (Math.sin(t * 0.31) * 0.09 + Math.sin(t * 0.77 + 1.3) * 0.045) + mind.look[0] * 0.3;
    var pitch = expr.pitch + wander * (Math.sin(t * 0.23 + 1) * 0.03 + Math.sin(t * 0.61) * 0.015) - mind.look[1] * 0.14;
    var roll = expr.roll + wander * (Math.sin(t * 0.29) * 0.025 + Math.sin(t * 0.83 + 2) * 0.012);
    if (mode === "working") yaw += sway * Math.sin(t * 1.6) * 0.04;
    if (mode === "idle" && t > mind.nextSigh) { mind.sighFrom = t; mind.nextSigh = t + between(25, 60); }
    var sigh = (t - mind.sighFrom) / 2.2;
    var sighing = sigh >= 0 && sigh <= 1 ? Math.sin(Math.PI * sigh) : 0;
    pitch += sway * sighing * 0.07;
    var nod = t - mind.nodFrom;
    if (nod >= 0 && nod < 1.2) pitch += sway * 0.1 * Math.sin(nod * 13) * Math.exp(-nod * 3.2);
    var shake = t - mind.shakeFrom;
    if (shake >= 0 && shake < 1.2) yaw += sway * 0.16 * Math.sin(shake * 15) * Math.exp(-shake * 3);
    var ease = 1 - Math.exp(-dt * 6);
    pose.yaw += (yaw - pose.yaw) * ease;
    pose.pitch += (pitch - pose.pitch) * ease;
    pose.roll += (roll - pose.roll) * ease;
    pose.jaw += (Math.max(mouth, expr.jaw + sighing * 0.1) - pose.jaw) * 0.45;

    // breathing: a slow swell and a small rise and fall
    var breath = sway * Math.sin(t * 1.45);
    return { breath: 1 + breath * 0.006 * (mode === "offline" ? 0.3 : 1), bob: breath * 0.004 };
  }

  // state: { rgb:[r,g,b], energy, mode, mouth (0..1), t (seconds), calm (bool) }
  function draw(ctx, cx, cy, size, state) {
    var t = state.t, mode = state.mode, calm = state.calm;
    var rgb = state.rgb;
    function rgba(a) { return "rgba(" + Math.round(rgb[0]) + "," + Math.round(rgb[1]) + "," + Math.round(rgb[2]) + "," + Math.max(0, Math.min(1, a)) + ")"; }
    var dim = mode === "offline" ? 0.4 : 1;

    // the mind decides the expression, the gaze, the blinks and the lean of the head
    var live = think(t, mode, state);
    cy += live.bob * size;
    size *= live.breath;
    var scale = size / 24;
    deform();
    project(cx, cy, scale);

    // facing of every triangle; the first frame decides which winding faces the viewer
    var nzs = new Float32Array(TRIS);
    var sum = 0;
    for (i = 0; i < TRIS; i += 1) {
      var a = TRI[i * 3], b = TRI[i * 3 + 1], c = TRI[i * 3 + 2];
      var ux = sx[b] - sx[a], uy = sy[b] - sy[a], uz = sz[b] - sz[a];
      var vx = sx[c] - sx[a], vy = sy[c] - sy[a], vz = sz[c] - sz[a];
      var nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx;
      var len = Math.sqrt(nx * nx + ny * ny + nz * nz) || 1;
      nzs[i] = nz / len;
      sum += nz / len;
    }
    if (!draw.decided) { frontFacing = sum >= 0 ? 1 : -1; draw.decided = true; }

    var BUCKETS = 6;
    var fills = [], edges = [], b2;
    for (b2 = 0; b2 < BUCKETS; b2 += 1) { fills.push([]); edges.push([]); }
    var backs = [], scans = [];
    var scanY = -9.4 + ((t * (mode === "working" ? 0.55 : 0.18)) % 1) * 19;
    for (i = 0; i < TRIS; i += 1) {
      var n = nzs[i] * frontFacing;
      if (n <= 0) { backs.push(i); continue; }
      var bucket = Math.min(BUCKETS - 1, Math.floor(Math.pow(n, 0.8) * BUCKETS));
      fills[bucket].push(i); edges[bucket].push(i);
      var cyl = (cur[TRI[i * 3] * 3 + 1] + cur[TRI[i * 3 + 1] * 3 + 1] + cur[TRI[i * 3 + 2] * 3 + 1]) / 3;
      if (Math.abs(cyl - scanY) < 0.38) scans.push(i);
    }

    function tri(index) {
      ctx.moveTo(rx[TRI[index * 3]], ry[TRI[index * 3]]);
      ctx.lineTo(rx[TRI[index * 3 + 1]], ry[TRI[index * 3 + 1]]);
      ctx.lineTo(rx[TRI[index * 3 + 2]], ry[TRI[index * 3 + 2]]);
      ctx.closePath();
    }

    ctx.save();
    ctx.globalCompositeOperation = "lighter";
    ctx.lineJoin = "round";

    // a soft backdrop, so the head sits in light rather than on a flat colour
    var back = ctx.createRadialGradient(cx, cy, size * 0.05, cx, cy, size * 0.62);
    back.addColorStop(0, rgba(0.16 * dim));
    back.addColorStop(1, rgba(0));
    ctx.fillStyle = back;
    ctx.beginPath(); ctx.arc(cx, cy, size * 0.62, 0, Math.PI * 2); ctx.fill();

    // the skull and the neck, behind the face
    ctx.lineWidth = 1;
    var fade = function (z) { return (0.1 + 0.18 * Math.max(0, Math.min(1, (z + 11) / 22))) * dim; };
    var k;
    for (k = 0; k < skull.length; k += 1) polyline(ctx, skull[k], cx, cy, scale, -0.4, fade, rgba);
    for (k = 0; k < neck.length; k += 1) {
      var nf = (function (kk) { return function (z) { return (0.2 - kk * 0.022) * dim; }; })(k);
      polyline(ctx, neck[k], cx, cy, scale, 99, nf, rgba);
    }

    // the far side of the face, faint, so the head reads as a volume
    ctx.beginPath();
    for (k = 0; k < backs.length; k += 1) tri(backs[k]);
    ctx.lineWidth = 0.6; ctx.strokeStyle = rgba(0.05 * dim); ctx.stroke();

    // the near side: a faint fill and a line per triangle, brighter where the surface faces the viewer
    for (b2 = 0; b2 < BUCKETS; b2 += 1) {
      var q = (b2 + 0.5) / BUCKETS;
      ctx.beginPath();
      for (k = 0; k < fills[b2].length; k += 1) tri(fills[b2][k]);
      ctx.fillStyle = rgba((0.03 + 0.2 * q * q) * dim);
      ctx.fill();
      ctx.lineWidth = 0.8;
      ctx.strokeStyle = rgba((0.1 + 0.5 * q) * dim);
      ctx.stroke();
    }

    // the scan plane passing over the face
    ctx.beginPath();
    for (k = 0; k < scans.length; k += 1) tri(scans[k]);
    ctx.lineWidth = 1.2; ctx.strokeStyle = rgba(0.95 * dim); ctx.stroke();
    ctx.fillStyle = rgba(0.22 * dim); ctx.fill();

    // the features, drawn heavier so the face has expression
    var feat = (0.55 + state.energy * 0.4) * dim;
    strip(ctx, EYE_L, true, 1.4, feat, rgba);
    strip(ctx, EYE_R, true, 1.4, feat, rgba);
    strip(ctx, BROW_L, false, 1.6, feat * 0.9, rgba);
    strip(ctx, BROW_R, false, 1.6, feat * 0.9, rgba);
    strip(ctx, LIPS_OUT, true, 1.5, feat, rgba);
    strip(ctx, LIPS_IN, true, 1.2, feat * (0.5 + pose.jaw), rgba);
    strip(ctx, NOSE, false, 1.2, feat * 0.55, rgba);

    eye(ctx, EYE_L, -1, scale, state, rgba);
    eye(ctx, EYE_R, 1, scale, state, rgba);

    ctx.restore();
  }

  // How the console steers the face. None of these move it by hand: they say what just happened, and the mind reacts.
  //   emote(name, seconds)  feel something for a while        nod() / shake()  yes / no
  //   mood(text)            the feeling an answer carries      speakMood(name)  the mood to hold while speaking
  function mood(text) {
    var s = String(text || "").toLowerCase();
    if (/\b(sorry|unable|cannot|can't|couldn't|could not|failed|error|unfortunately|problem|refused|denied)\b/.test(s)) return "concerned";
    if (/\b(haha|funny|joke|lol|amusing)\b/.test(s)) return "amused";
    if (/!|\b(great|glad|happy|love|awesome|nice|congrat|welcome|delighted|good evening|good morning|hello|done)\b/.test(s)) return "pleased";
    if (/\?\s*$/.test(s)) return "curious";
    if (/\b(perhaps|might|however|depends|consider|not sure|unclear)\b/.test(s)) return "thinking";
    return "pleasant";
  }
  function emote(name, seconds) { if (EMO[name]) mind.requested = { name: name, seconds: seconds || 2.5 }; }
  function nod() { mind.nodFrom = mind.lastT; }
  function shake() { mind.shakeFrom = mind.lastT; }
  function speakMood(name) { if (EMO[name]) mind.speakMood = name; }
  function current() { return mind.emotion; }

  window.JarvisHead = {
    draw: draw, triangles: TRIS, vertices: N, emotions: Object.keys(EMO),
    mood: mood, emote: emote, nod: nod, shake: shake, speakMood: speakMood, current: current
  };
})();
