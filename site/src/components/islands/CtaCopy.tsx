import { IslandRoot } from './IslandRoot';
import { CopyButton } from '../morphin/border-button/CopyButton';
// The closing CTA's Copy: the Install section's button, so both copy, animate and announce alike.
export default function CtaCopy({ text }: { text: string }) {
 return <IslandRoot><CopyButton text={text} label="install command" /></IslandRoot>;
}
