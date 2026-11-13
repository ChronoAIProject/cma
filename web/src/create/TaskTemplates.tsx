import { Button } from '../ui/Button';
import {
  IconCodeBracketsOutline16,
  IconFileOutline16,
  IconGlobeOutline16,
  IconSearchOutline16,
  IconTableOutline16,
} from '../ui/icons';

const templates = [
  {
    name: 'Research a topic',
    text: 'Research [topic] and summarize the key findings, with links to sources.',
    icon: <IconSearchOutline16 />,
  },
  {
    name: 'Analyze a spreadsheet',
    text: 'Analyze the spreadsheet at [link] and point out trends, outliers and what to do next.',
    icon: <IconTableOutline16 />,
  },
  {
    name: 'Write a document',
    text: 'Write a [type of document] about [subject] for [audience].',
    icon: <IconFileOutline16 />,
  },
  {
    name: 'Build a website',
    text: 'Build a one-page website for [project] and share a preview link.',
    icon: <IconGlobeOutline16 />,
  },
  {
    name: 'Write code',
    text: 'Write a [language] script that [does what].',
    icon: <IconCodeBracketsOutline16 />,
  },
];

export function TaskTemplates({ onSelect }: { readonly onSelect: (text: string) => void }) {
  return (
    <div className="task-home__templates" role="group" aria-label="Task templates">
      {templates.map(({ name, text, icon }) => (
        <Button
          key={name}
          placement="task-home__template"
          variant="outline"
          icon={icon}
          onClick={() => onSelect(text)}
        >
          {name}
        </Button>
      ))}
    </div>
  );
}
