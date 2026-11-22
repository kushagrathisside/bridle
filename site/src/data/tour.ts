// The product walkthrough. Each item is one real screenshot of the Bridle window and the part of it the text is about. The window
// is the shipped bridle.exe on a clone of more-itertools (MIT), driven through its own commands; see site/README.md.
// Regions are fractions of the image (x, y, width, height), measured on the screenshot itself. `crop` is the region shown on a
// phone, where the whole window would be too small to read.
export type Region = readonly [x: number, y: number, w: number, h: number];

export interface TourItem {
  id: string;
  title: string;
  text: string;
  /** The file name in src/assets/shots, without the extension. */
  image: string;
  region: Region;
  crop?: Region;
  more?: { label: string; href: string };
}

export const tour: TourItem[] = [
  {
    id: 'chat',
    title: 'Every call is a card',
    text: 'The agent’s tool calls appear in the chat as it makes them: what it ran, on what, and whether it finished. Here it maps a library and finds where a function is defined and used, with the code tools. Edits show as a coloured diff in the conversation.',
    image: 'tour-chat',
    region: [0.535, 0.05, 0.43, 0.9],
    more: { label: 'The desktop window', href: 'guides/desktop' },
  },
  {
    id: 'blocked',
    title: 'A refusal names its rule',
    text: 'Asked to install packages, the agent tries pip. The policy stops it, the card says so and names the rule, and the agent reads the same message and changes course. The next attempt, a Python program, is refused as well.',
    image: 'tour-blocked',
    region: [0.455, 0.085, 0.51, 0.34],
    more: { label: 'How decisions are made', href: 'concepts/how-decisions-are-made' },
  },
  {
    id: 'editor',
    title: 'Code, with the change beside it',
    text: 'A Monaco editor with tabs, and diff tabs that compare the working copy with the last commit. The function the agent just added is in green. A file the agent changes updates by itself, and the explorer marks what changed.',
    image: 'tour-diff',
    region: [0.218, 0.04, 0.318, 0.66],
  },
  {
    id: 'team',
    title: 'A team that answers to the same policy',
    text: 'Tag a role in the chat and the request becomes issues the members work. Here a Project Manager plans the change, and each member’s messages and tool calls appear under their name. Every call is checked like any other.',
    image: 'tour-team',
    region: [0.535, 0.05, 0.465, 0.9],
    more: { label: 'Team and board', href: 'guides/team-and-board' },
  },
  {
    id: 'board',
    title: 'A board of issues',
    text: 'Issues move through backlog, ready, in progress, review and done. Each one keeps its assignee, its acceptance criteria and the comments left by the member who worked it, such as a QA Lead’s review.',
    image: 'tour-board',
    region: [0.03, 0.054, 0.94, 0.9],
    crop: [0.31, 0.11, 0.41, 0.45],
  },
  {
    id: 'activity',
    title: 'The decision log, live',
    text: 'The Activity panel lists each tool call with the policy’s verdict and rule, newest first. The same lines are in .bridle/tool-events.jsonl.',
    image: 'tour-activity',
    region: [0.173, 0.49, 0.51, 0.49],
    more: { label: 'Tasks and loops', href: 'concepts/tasks-and-loops' },
  },
  {
    id: 'models',
    title: 'Models, started and checked here',
    text: 'Each model slot shows its state, its loopback endpoint, its runtime and its model file, with Start, Stop and a health check. A GPU model that will not fit is refused before it loads.',
    image: 'tour-models',
    region: [0.035, 0.05, 0.17, 0.92],
    more: { label: 'Serve the models', href: 'guides/serve-models' },
  },
];
