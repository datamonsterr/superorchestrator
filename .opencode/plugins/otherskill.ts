// OtherSkill managed adapter
import {createOpenCodePlugin} from "file:///home/dat/.local/share/otherskill/installs/release.K6W9tj7O/plugins/otherskill-opencode.ts";
export const OtherSkill = async ({directory}:{directory:string}) => createOpenCodePlugin({command:["/home/dat/.local/share/otherskill/bin/otherskill","find","--agent","opencode","--home","/home/dat","--config","/home/dat/dev/superochestrator/otherskill.config.json","--model","inception/mercury-decide:free"],cwd:directory,timeoutMs:65750});
