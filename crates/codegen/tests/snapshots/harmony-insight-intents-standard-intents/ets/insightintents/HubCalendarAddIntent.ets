// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `calendar.add` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { CalendarAddParams, HubToolHandlers } from '../appmcp/HubTools';

/**
 * 在日历中新建一个日程
 *
 * 工具 `calendar.add`「新建日程」，风险：write
 */
@InsightIntentEntry({
  intentName: "HubCalendarAdd",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "新建日程",
  displayDescription: "在日历中新建一个日程",
  llmDescription: "在日历中新建一个日程",
  keywords: ["新建日程", "calendar.add"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "在日历中新建一个日程",
    "properties": {
      "allDay": {
        "type": "boolean"
      },
      "end": {
        "type": "string"
      },
      "location": {
        "type": "string"
      },
      "start": {
        "type": "string"
      },
      "title": {
        "type": "string"
      }
    },
    "required": [
      "title",
      "start"
    ],
    "type": "object"
  },
})
export default class HubCalendarAddIntent extends InsightIntentEntryExecutor<string> {
  public title?: string;
  /** 格式：date-time */
  public start?: string;
  /** 格式：date-time */
  public end?: string;
  public allDay?: boolean;
  public location?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const titleValue = this.title;
    if (titleValue === undefined) {
      return HubIntentRuntime.missing("title");
    }
    const startValue = this.start;
    if (startValue === undefined) {
      return HubIntentRuntime.missing("start");
    }
    const params: CalendarAddParams = {
      title: titleValue,
      start: startValue,
      end: this.end,
      allDay: this.allDay,
      location: this.location,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.calendarAdd(params));
  }
}
